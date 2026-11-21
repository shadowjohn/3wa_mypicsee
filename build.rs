fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("assets/app.ico")
        .set("ProductName", "3wa_mypicsee")
        .set("FileDescription", "3wa_mypicsee - Offline photo viewer")
        .compile()
        .expect("embed Windows icon and version metadata");
}
