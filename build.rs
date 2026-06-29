fn main() {
    println!("cargo:rerun-if-changed=assets/sidetone.ico");

    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/sidetone.ico");
        resource.compile().expect("failed to embed Windows icon");
    }
}
