fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("compilar la UI");

    // Icono y datos de versión del .exe (Explorador, barra de tareas, accesos directos).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/swiftbeat.ico")
            .set("ProductName", "Swiftbeat")
            .set("FileDescription", "Swiftbeat")
            .set("InternalName", "swiftbeat.exe")
            .set("OriginalFilename", "swiftbeat.exe")
            .set("LegalCopyright", "MIT License");
        res.compile().expect("incrustar el icono");
    }
}
