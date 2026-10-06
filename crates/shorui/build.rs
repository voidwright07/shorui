fn main() {
    // Windows shows the icon stored in the exe as resource 1.
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the app icon: {e}");
        }
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
