fn main() {
    if cfg!(target_os = "windows") {
        let mut res = winres::WindowsResource::new();
        res.set_icon("resources/book.ico");
        res.set_manifest_file("resources/manifest.xml");
        res.compile().unwrap();
    }
}
