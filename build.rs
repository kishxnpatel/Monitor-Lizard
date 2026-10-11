fn main() {
    // Fluent = the Windows 11 look. It follows the system light/dark setting.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("UI failed to compile");
}
