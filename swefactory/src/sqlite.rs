//! Minimal read-only SQLite file reader (ticket #88 cutover import).
//!
//! Pure Rust, zero new dependencies: parses just enough of the SQLite
//! database file format to read small table b-trees — the bash
//! dispatcher's `attempts` and `releases` history. This is NOT a general
//! SQLite engine: no indexes, no `WITHOUT ROWID` tables, no triggers, no
//! WAL frames, no freelist resurrection. Anything outside the narrow path
//! it understands is a clean error naming what it found, never a silent
//! misread. The importer only ever reads the source `.db` through here.
//!
//! Format reference: https://www.sqlite.org/fileformat.html

use std::collections::HashMap;

/// One cell value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
    Blob(Vec<u8>),
}

/// One table row: column names in schema order plus one value each.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub columns: Vec<String>,
    pub values: Vec<Value>,
}

impl Row {
    fn pos(&self, col: &str) -> Result<usize, String> {
        self.columns
            .iter()
            .position(|c| c == col)
            .ok_or_else(|| format!("cutover: no column '{col}' in history row"))
    }

    /// Required integer column (bash `INTEGER` fields).
    pub fn req_int(&self, col: &str) -> Result<i64, String> {
        match self.values.get(self.pos(col)?) {
            Some(Value::Int(i)) => Ok(*i),
            other => Err(format!(
                "cutover: column '{col}' holds {:?}, want an integer",
                kind_of(other)
            )),
        }
    }

    /// Required non-negative integer column (ids, tickets, attempts).
    pub fn req_u64(&self, col: &str) -> Result<u64, String> {
        let i = self.req_int(col)?;
        u64::try_from(i)
            .map_err(|_| format!("cutover: column '{col}' holds a negative integer ({i})"))
    }

    /// Optional integer column: `NULL` (never set) reads as `None`.
    pub fn opt_int(&self, col: &str) -> Result<Option<i64>, String> {
        match self.values.get(self.pos(col)?) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Int(i)) => Ok(Some(*i)),
            other => Err(format!(
                "cutover: column '{col}' holds {:?}, want an integer or NULL",
                kind_of(other)
            )),
        }
    }

    /// Text column: `NULL` (never set) reads as empty, matching the
    /// empty-string defaults a pre-checkpoint Rust ledger carries. A
    /// stray integer coerces losslessly; floats and blobs are refused
    /// loudly instead of being stringified into the ledger.
    pub fn text(&self, col: &str) -> Result<String, String> {
        match self.values.get(self.pos(col)?) {
            None | Some(Value::Null) => Ok(String::new()),
            Some(Value::Text(s)) => Ok(s.clone()),
            Some(Value::Int(i)) => Ok(i.to_string()),
            other => Err(format!(
                "cutover: column '{col}' holds {:?}, want text or NULL",
                kind_of(other)
            )),
        }
    }

    /// Text column that may not exist at all (e.g. `train_branch` on a
    /// history written before the bash migration added it): a missing
    /// column reads as empty, exactly like a `NULL` in every row.
    pub fn maybe_text(&self, col: &str) -> Result<String, String> {
        if self.columns.iter().any(|c| c == col) {
            self.text(col)
        } else {
            Ok(String::new())
        }
    }

    /// Required text column: present and non-empty (bash `NOT NULL`
    /// fields such as `status`).
    pub fn req_text(&self, col: &str) -> Result<String, String> {
        let s = self.text(col)?;
        if s.is_empty() {
            return Err(format!("cutover: column '{col}' is empty, want a value"));
        }
        Ok(s)
    }
}

fn kind_of(v: Option<&Value>) -> &'static str {
    match v {
        None => "nothing",
        Some(Value::Null) => "NULL",
        Some(Value::Int(_)) => "integer",
        Some(Value::Float(_)) => "float",
        Some(Value::Text(_)) => "text",
        Some(Value::Blob(_)) => "blob",
    }
}

/// Visitor over one table b-tree: each leaf cell's rowid plus its record
/// payload, in rowid order.
type RecordVisit<'a> = dyn FnMut(u64, &[u8]) -> Result<(), String> + 'a;

/// An opened database image: the raw bytes plus the header facts every
/// page walk needs. Built from bytes the caller already holds so the
/// importer reads the source file exactly once.
pub struct Db {
    data: Vec<u8>,
    page_size: usize,
    reserved: usize,
    encoding: u32,
}

impl Db {
    /// Parse a database image. Refuses anything that is not a SQLite
    /// database file, with the reason named.
    pub fn parse(data: Vec<u8>) -> Result<Db, String> {
        if data.len() < 100 || &data[0..16] != b"SQLite format 3\0" {
            return Err("cutover: not a SQLite database file (bad magic)".to_string());
        }
        let raw = u16::from_be_bytes([data[16], data[17]]) as usize;
        let page_size = if raw == 1 { 65536 } else { raw };
        if page_size < 512 || data.len() < page_size {
            return Err(format!(
                "cutover: bad SQLite page size ({page_size}) for a {}-byte file",
                data.len()
            ));
        }
        let encoding = u32::from_be_bytes([data[56], data[57], data[58], data[59]]);
        if !(1..=3).contains(&encoding) {
            return Err(format!(
                "cutover: unknown SQLite text encoding ({encoding})"
            ));
        }
        let reserved = data[20] as usize;
        Ok(Db {
            data,
            page_size,
            reserved,
            encoding,
        })
    }

    fn usable(&self, pgno: u32) -> usize {
        self.page_size - self.reserved - if pgno == 1 { 100 } else { 0 }
    }

    fn page_off(&self, pgno: u32) -> Result<usize, String> {
        if pgno == 0 {
            return Err("cutover: corrupt b-tree (page 0 reference)".to_string());
        }
        let off = (pgno as usize - 1) * self.page_size;
        if off + self.page_size > self.data.len() {
            return Err(format!("cutover: corrupt b-tree (page {pgno} past end)"));
        }
        Ok(off)
    }

    fn u16_at(&self, at: usize) -> Result<usize, String> {
        let b = self
            .data
            .get(at..at + 2)
            .ok_or_else(|| "cutover: corrupt b-tree (truncated header)".to_string())?;
        Ok(u16::from_be_bytes([b[0], b[1]]) as usize)
    }

    fn u32_at(&self, at: usize) -> Result<u32, String> {
        let b = self
            .data
            .get(at..at + 4)
            .ok_or_else(|| "cutover: corrupt b-tree (truncated pointer)".to_string())?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Read every row of a table b-tree in rowid order. A bare
    /// `INTEGER PRIMARY KEY` column is a rowid alias: SQLite stores it
    /// as NULL in the record, so a NULL there is filled from the rowid
    /// (this is exactly how the bash `id` columns read back their ids).
    pub fn table(&self, name: &str) -> Result<Vec<Row>, String> {
        let (rootpage, sql) = self.schema_entry(name)?;
        let columns = parse_create_columns(&sql, name)?;
        let pk = rowid_alias(&sql, &columns);
        let mut rows = Vec::new();
        self.walk(rootpage, &mut |rowid, record| {
            let mut row = decode_record(&columns, record, self.encoding, name)?;
            if let Some(pk_col) = pk.as_deref() {
                if let Some(i) = columns.iter().position(|c| c == pk_col) {
                    if row.values[i] == Value::Null {
                        row.values[i] = Value::Int(rowid as i64);
                    }
                }
            }
            rows.push(row);
            Ok(())
        })?;
        Ok(rows)
    }

    /// Root page plus `CREATE TABLE` text for one table, from the
    /// `sqlite_schema` b-tree rooted at page 1 (fixed column order
    /// `type, name, tbl_name, rootpage, sql` per the file format).
    fn schema_entry(&self, name: &str) -> Result<(u32, String), String> {
        let mut found: Option<(u32, String)> = None;
        self.walk(1, &mut |_rowid, record| {
            let cols = vec![
                "type".to_string(),
                "name".to_string(),
                "tbl_name".to_string(),
                "rootpage".to_string(),
                "sql".to_string(),
            ];
            let row = decode_record(&cols, record, self.encoding, "sqlite_schema")?;
            let is_table = matches!(row.values.first(), Some(Value::Text(t)) if t == "table");
            let is_name = matches!(row.values.get(1), Some(Value::Text(n)) if n == name);
            if is_table && is_name {
                let rootpage = match row.values.get(3) {
                    Some(Value::Int(i)) if *i > 0 => *i as u32,
                    _ => return Err(format!("cutover: table '{name}' has no b-tree root page")),
                };
                let sql = match row.values.get(4) {
                    Some(Value::Text(s)) => s.clone(),
                    _ => return Err(format!("cutover: table '{name}' has no CREATE TABLE text")),
                };
                found = Some((rootpage, sql));
            }
            Ok(())
        })?;
        found.ok_or_else(|| format!("cutover: no table '{name}' in the SQLite history"))
    }

    /// Walk a table b-tree in rowid order, calling `f` with each leaf
    /// cell's rowid plus its record payload.
    fn walk(&self, pgno: u32, f: &mut RecordVisit<'_>) -> Result<(), String> {
        let base = self.page_off(pgno)?;
        let hdr = base + if pgno == 1 { 100 } else { 0 };
        let kind = *self
            .data
            .get(hdr)
            .ok_or_else(|| "cutover: corrupt b-tree (truncated page)".to_string())?;
        let ncells = self.u16_at(hdr + 3)?;
        match kind {
            0x05 => {
                let rightmost = self.u32_at(hdr + 8)?;
                for i in 0..ncells {
                    let ptr = self.u16_at(hdr + 12 + 2 * i)?;
                    let cell = self.cell_at(base, ptr)?;
                    let child = self.u32_at(cell).map_err(|_| {
                        "cutover: corrupt b-tree (truncated interior cell)".to_string()
                    })?;
                    self.walk(child, f)?;
                }
                self.walk(rightmost, f)?;
                Ok(())
            }
            0x0D => {
                for i in 0..ncells {
                    let ptr = self.u16_at(hdr + 8 + 2 * i)?;
                    let (rowid, record) = self.leaf_record(base, pgno, ptr)?;
                    f(rowid, &record)?;
                }
                Ok(())
            }
            _ => Err(format!(
                "cutover: unsupported b-tree page type {kind:#04x} (page {pgno})"
            )),
        }
    }

    fn cell_at(&self, base: usize, ptr: usize) -> Result<usize, String> {
        let cell = base + ptr;
        if ptr >= self.page_size || cell >= self.data.len() {
            return Err("cutover: corrupt b-tree (cell pointer past end)".to_string());
        }
        Ok(cell)
    }

    /// One leaf-table cell: varint payload length, varint rowid, then the
    /// record payload (following the overflow chain when spilled).
    /// Returns the rowid alongside the payload: a bare `INTEGER PRIMARY
    /// KEY` column is a rowid alias stored as NULL in the record.
    fn leaf_record(&self, base: usize, pgno: u32, ptr: usize) -> Result<(u64, Vec<u8>), String> {
        let cell = self.cell_at(base, ptr)?;
        let (payload_len, pos) = varint(&self.data, cell)
            .map_err(|_| "cutover: corrupt b-tree (truncated leaf cell)".to_string())?;
        let payload_len = payload_len as usize;
        let (rowid, pos) = varint(&self.data, pos)
            .map_err(|_| "cutover: corrupt b-tree (truncated leaf cell)".to_string())?;
        let usable = self.usable(pgno);
        let local = leaf_local(payload_len, usable);
        let page_end = base + self.page_size;
        if pos + local.min(payload_len) > page_end {
            return Err("cutover: corrupt b-tree (cell past page end)".to_string());
        }
        let mut out = Vec::with_capacity(payload_len);
        let take = local.min(payload_len);
        out.extend_from_slice(&self.data[pos..pos + take]);
        let mut remaining = payload_len - take;
        if remaining > 0 {
            // The last 4 local bytes are the first overflow page number.
            if out.len() < 4 {
                return Err("cutover: corrupt b-tree (missing overflow link)".to_string());
            }
            let mut next = u32::from_be_bytes([
                out[out.len() - 4],
                out[out.len() - 3],
                out[out.len() - 2],
                out[out.len() - 1],
            ]) as usize;
            out.truncate(out.len() - 4);
            remaining += 4;
            while remaining > 0 {
                if next == 0 {
                    return Err("cutover: corrupt b-tree (short overflow chain)".to_string());
                }
                let off = self.page_off(next as u32)?;
                next = self.u32_at(off)? as usize;
                let chunk = (usable - 4).min(remaining);
                if off + 4 + chunk > off + self.page_size {
                    return Err("cutover: corrupt b-tree (overflow past page end)".to_string());
                }
                out.extend_from_slice(&self.data[off + 4..off + 4 + chunk]);
                remaining -= chunk;
            }
        }
        if out.len() != payload_len {
            return Err("cutover: corrupt b-tree (payload length mismatch)".to_string());
        }
        Ok((rowid, out))
    }
}

/// The single-column `INTEGER PRIMARY KEY` of a `CREATE TABLE` — the
/// rowid alias — if it names one of the parsed columns. Matching is
/// deliberately narrow (exactly `INTEGER PRIMARY KEY` on one column):
/// anything else keeps its stored value, never a substituted rowid.
fn rowid_alias(sql: &str, columns: &[String]) -> Option<String> {
    let open = sql.find('(')?;
    let body = sql.get(open + 1..)?;
    // Reuse the top-level split by cutting at the matching close paren.
    let mut depth = 0;
    let mut end = body.len();
    for (i, c) in body.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => {
                end = i;
                break;
            }
            ')' => depth -= 1,
            _ => {}
        }
    }
    let body = &body[..end];
    // Split top-level commas the same way the column parser does.
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut bracket = false;
    let mut paren = 0;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            cur.push(c);
            if c == q {
                if chars.peek() == Some(&q) {
                    cur.push(chars.next().unwrap());
                } else {
                    quote = None;
                }
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => {
                quote = Some(c);
                cur.push(c);
            }
            '[' => {
                bracket = true;
                cur.push(c);
            }
            ']' => {
                bracket = false;
                cur.push(c);
            }
            '(' => {
                paren += 1;
                cur.push(c);
            }
            ')' => {
                paren -= 1;
                cur.push(c);
            }
            ',' if !bracket && paren == 0 => {
                parts.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_string());
    }
    for part in &parts {
        let name = column_name(part);
        // The definition past the name must read `INTEGER PRIMARY KEY`
        // (in that order, adjacent): the exact alias spelling.
        let rest: String = part
            .trim()
            .get(name.len().min(part.trim().len())..)
            .unwrap_or_default()
            .trim_matches(|c: char| c == '"' || c == '\'' || c == '`' || c == ']')
            .trim_start_matches('[')
            .to_string();
        let words: Vec<String> = rest
            .split_whitespace()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_uppercase()
            })
            .collect();
        if words.len() >= 3
            && words[0] == "INTEGER"
            && words[1] == "PRIMARY"
            && words[2] == "KEY"
            && columns.iter().any(|c| c == &name)
        {
            return Some(name);
        }
    }
    None
}

/// How many payload bytes a leaf-table cell keeps locally (file-format
/// overflow rule): small payloads sit entirely on the leaf page, larger
/// ones spill onto overflow pages linked from the last 4 local bytes.
fn leaf_local(payload_len: usize, usable: usize) -> usize {
    let x = usable - 35;
    if payload_len <= x {
        return payload_len;
    }
    let m = ((usable - 12) * 32 / 255) - 23;
    let k = m + ((payload_len - m) % (usable - 4));
    if k <= x {
        k
    } else {
        m
    }
}

/// SQLite variable-length integer: bytes 1-8 carry 7 bits each while the
/// high bit is set; a 9th byte, when present, carries a full 8 bits.
fn varint(data: &[u8], pos: usize) -> Result<(u64, usize), String> {
    let mut v: u64 = 0;
    let mut p = pos;
    for i in 0..9 {
        let b = *data
            .get(p)
            .ok_or_else(|| "cutover: truncated varint".to_string())?;
        p += 1;
        if i == 8 {
            v = (v << 8) | b as u64;
            break;
        }
        v = (v << 7) | (b & 0x7F) as u64;
        if b & 0x80 == 0 {
            break;
        }
    }
    Ok((v, p))
}

/// Big-endian two's-complement integer of 1-8 bytes, sign-extended.
fn be_int(bytes: &[u8]) -> i64 {
    let mut v: i64 = if bytes[0] & 0x80 != 0 { -1 } else { 0 };
    for b in bytes {
        v = (v << 8) | *b as i64;
    }
    v
}

fn decode_text(bytes: &[u8], encoding: u32) -> Result<String, String> {
    match encoding {
        1 => String::from_utf8(bytes.to_vec())
            .map_err(|e| format!("cutover: invalid UTF-8 text in history: {e}")),
        2 | 3 => {
            if !bytes.len().is_multiple_of(2) {
                return Err("cutover: odd-length UTF-16 text in history".to_string());
            }
            let units: Vec<u16> = bytes
                .chunks(2)
                .map(|c| {
                    if encoding == 2 {
                        u16::from_le_bytes([c[0], c[1]])
                    } else {
                        u16::from_be_bytes([c[0], c[1]])
                    }
                })
                .collect();
            char::decode_utf16(units)
                .collect::<Result<String, _>>()
                .map_err(|e| format!("cutover: invalid UTF-16 text in history: {e:?}"))
        }
        _ => Err(format!(
            "cutover: unknown SQLite text encoding ({encoding})"
        )),
    }
}

/// Decode one record payload into a row: the header carries one serial
/// type per column, then the values follow in order. Rows written before
/// a trailing `ALTER TABLE ... ADD COLUMN` carry fewer values; the
/// missing tail reads as `NULL`, exactly like SQLite itself reports.
fn decode_record(
    columns: &[String],
    record: &[u8],
    encoding: u32,
    table: &str,
) -> Result<Row, String> {
    let corrupt = || format!("cutover: corrupt record in table '{table}'");
    let (header_len, mut pos) = varint(record, 0).map_err(|_| corrupt())?;
    let header_end = header_len as usize;
    if header_end > record.len() || header_end == 0 {
        return Err(corrupt());
    }
    let mut serials = Vec::new();
    while pos < header_end {
        let (s, next) = varint(record, pos).map_err(|_| corrupt())?;
        serials.push(s);
        pos = next;
    }
    let mut values = Vec::with_capacity(serials.len());
    for s in &serials {
        let (kind, len) = match *s {
            0 => ("null", 0),
            1 => ("int", 1),
            2 => ("int", 2),
            3 => ("int", 3),
            4 => ("int", 4),
            5 => ("int", 6),
            6 => ("int", 8),
            7 => ("float", 8),
            8 => ("const0", 0),
            9 => ("const1", 0),
            10 | 11 => return Err(format!("cutover: reserved serial type {s} in '{table}'")),
            n if n % 2 == 0 => ("blob", ((n - 12) / 2) as usize),
            n => ("text", ((n - 13) / 2) as usize),
        };
        let bytes = record.get(pos..pos + len).ok_or_else(corrupt)?;
        pos += len;
        values.push(match kind {
            "null" => Value::Null,
            "const0" => Value::Int(0),
            "const1" => Value::Int(1),
            "int" => Value::Int(be_int(bytes)),
            "float" => Value::Float(f64::from_be_bytes(bytes.try_into().map_err(|_| corrupt())?)),
            "blob" => Value::Blob(bytes.to_vec()),
            _ => Value::Text(decode_text(bytes, encoding).map_err(|_| corrupt())?),
        });
    }
    if pos != record.len() {
        return Err(corrupt());
    }
    if values.len() > columns.len() {
        return Err(format!(
            "cutover: table '{table}' row has {} values for {} columns",
            values.len(),
            columns.len()
        ));
    }
    while values.len() < columns.len() {
        values.push(Value::Null);
    }
    Ok(Row {
        columns: columns.to_vec(),
        values,
    })
}

/// Column names in schema order from a `CREATE TABLE` statement: splits
/// the top-level comma-separated definitions (respecting quotes and
/// nested parentheses) and drops table constraints. Never parses column
/// types — the record serial types carry the values.
fn parse_create_columns(sql: &str, table: &str) -> Result<Vec<String>, String> {
    let err = || format!("cutover: cannot parse CREATE TABLE for '{table}'");
    let open = sql.find('(').ok_or_else(err)?;
    let mut depth = 0;
    let mut close = None;
    for (i, c) in sql.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &sql[open + 1..close.ok_or_else(err)?];
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut bracket = false;
    let mut paren = 0;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            cur.push(c);
            if c == q {
                if chars.peek() == Some(&q) {
                    cur.push(chars.next().unwrap());
                } else {
                    quote = None;
                }
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => {
                quote = Some(c);
                cur.push(c);
            }
            '[' => {
                bracket = true;
                cur.push(c);
            }
            ']' => {
                bracket = false;
                cur.push(c);
            }
            '(' => {
                paren += 1;
                cur.push(c);
            }
            ')' => {
                paren -= 1;
                cur.push(c);
            }
            ',' if !bracket && paren == 0 => {
                parts.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_string());
    }
    let mut columns = Vec::new();
    for part in parts {
        let upper = part.to_uppercase();
        let head: String = upper
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        if ["CONSTRAINT", "PRIMARY", "FOREIGN", "UNIQUE", "CHECK"].contains(&head.as_str()) {
            continue;
        }
        columns.push(column_name(&part));
    }
    if columns.is_empty() {
        return Err(err());
    }
    Ok(columns)
}

/// First token of a column definition, unquoted.
fn column_name(def: &str) -> String {
    let def = def.trim();
    let mut chars = def.chars();
    let first = chars.next().unwrap_or_default();
    match first {
        '"' | '\'' | '`' => {
            let mut name = String::new();
            for c in chars {
                if c == first {
                    break;
                }
                name.push(c);
            }
            name
        }
        '[' => {
            let mut name = String::new();
            for c in chars {
                if c == ']' {
                    break;
                }
                name.push(c);
            }
            name
        }
        _ => def
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string(),
    }
}

/// Schema-name lookup shared by the importer: which tables exist.
#[allow(dead_code)]
pub fn table_names(data: &[u8]) -> Result<Vec<String>, String> {
    let db = Db::parse(data.to_vec())?;
    let mut names = Vec::new();
    let mut seen = HashMap::new();
    db.walk(1, &mut |_rowid, record| {
        let cols = vec![
            "type".to_string(),
            "name".to_string(),
            "tbl_name".to_string(),
            "rootpage".to_string(),
            "sql".to_string(),
        ];
        let row = decode_record(&cols, record, db.encoding, "sqlite_schema")?;
        if let (Some(Value::Text(t)), Some(Value::Text(n))) =
            (row.values.first(), row.values.get(1))
        {
            if t == "table" && !seen.contains_key(n) {
                seen.insert(n.clone(), ());
                names.push(n.clone());
            }
        }
        Ok(())
    })?;
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_single_and_multi_byte() {
        assert_eq!(varint(&[0x00], 0).unwrap(), (0, 1));
        assert_eq!(varint(&[0x7F], 0).unwrap(), (127, 1));
        // 129 = 0x81 0x01
        assert_eq!(varint(&[0x81, 0x01], 0).unwrap(), (129, 2));
        // 9-byte maximum fills all 64 bits.
        let big = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        assert_eq!(varint(&big, 0).unwrap(), (u64::MAX, 9));
        assert!(varint(&[0x81], 0).is_err());
    }

    #[test]
    fn signed_big_endian_extension() {
        assert_eq!(be_int(&[0x7F]), 127);
        assert_eq!(be_int(&[0x80]), -128);
        assert_eq!(be_int(&[0xFF, 0x80]), -128);
        assert_eq!(be_int(&[0xFF, 0xFF, 0x80]), -128);
        assert_eq!(be_int(&[0x01, 0x00]), 256);
    }

    #[test]
    fn record_with_null_text_and_int() {
        // Columns (id, status, exit_code): header [3 bytes: len=4? ...].
        // header_len=4, serials [1 (1-byte int), 13+2*6=25 (6-char text), 0 (null)].
        let record = vec![
            0x04, 0x01, 0x19, 0x00, // header
            0x07, // id = 7
            b'h', b'i', b'!', b'!', b'!', b'!', // status
        ];
        let cols = ["id", "status", "exit_code"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let row = decode_record(&cols, &record, 1, "t").unwrap();
        assert_eq!(
            row.values,
            vec![
                Value::Int(7),
                Value::Text("hi!!!!".to_string()),
                Value::Null
            ]
        );
    }

    #[test]
    fn short_rows_pad_with_null() {
        // A row written before ALTER TABLE ... ADD COLUMN carries fewer
        // values; the missing tail reads as NULL.
        let record = vec![0x03, 0x01, 0x00, 0x05];
        let cols = ["id", "status", "train_branch"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let row = decode_record(&cols, &record, 1, "t").unwrap();
        assert_eq!(row.values, vec![Value::Int(5), Value::Null, Value::Null]);
    }

    #[test]
    fn create_table_parsing_skips_constraints() {
        let sql = "CREATE TABLE attempts(\n    id INTEGER PRIMARY KEY, ticket INTEGER NOT NULL,\n    \"weird name\" TEXT, [bracketed] TEXT,\n    PRIMARY KEY (id), CHECK (ticket > 0))";
        let cols = parse_create_columns(sql, "attempts").unwrap();
        assert_eq!(cols, vec!["id", "ticket", "weird name", "bracketed"]);
    }

    #[test]
    fn leaf_spill_boundary_math() {
        // 4096-byte pages, no reserved space: everything close fits.
        assert_eq!(leaf_local(100, 4096), 100);
        assert_eq!(leaf_local(4061, 4096), 4061);
        // Past U-35 the payload spills; the local share follows the
        // documented M/K rule instead of being silently truncated.
        let m = ((4096 - 12) * 32 / 255) - 23;
        assert_eq!(leaf_local(5000, 4096), m + ((5000 - m) % (4096 - 4)));
    }

    #[test]
    fn missing_table_names_itself() {
        let db = Db::parse(minimal_db_with_schema(&[])).unwrap();
        let err = db.table("attempts").unwrap_err();
        assert!(err.contains("no table 'attempts'"), "got: {err}");
    }

    /// Builds the smallest valid database image: header + page 1 holding
    /// an empty sqlite_schema leaf b-tree.
    fn minimal_db_with_schema(_entries: &[&str]) -> Vec<u8> {
        let page_size = 512;
        let mut data = vec![0u8; page_size];
        data[0..16].copy_from_slice(b"SQLite format 3\0");
        data[16..18].copy_from_slice(&512u16.to_be_bytes());
        data[18] = 1;
        data[19] = 1;
        data[56..60].copy_from_slice(&1u32.to_be_bytes());
        // Page-1 b-tree header at offset 100: leaf table, zero cells.
        data[100] = 0x0D;
        data[103..105].copy_from_slice(&0u16.to_be_bytes());
        data[105..107].copy_from_slice(&(page_size as u16).to_be_bytes());
        data
    }
}
