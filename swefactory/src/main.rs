/// Thin CLI wrapper (ticket #28, spec #27 prefactor): all command
/// implementations live in `factory_lib` so future Axum handlers call the
/// same functions the CLI calls. Zero behavior change by construction:
///
/// the only code path is `factory_lib::run()`.
fn main() {
    std::process::exit(factory_lib::run());
}
