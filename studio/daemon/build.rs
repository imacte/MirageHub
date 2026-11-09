fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo::rerun-if-changed=../../assets/mirage-hub.ico");
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("../../assets/mirage-hub.ico");
    resource.set(
        "FileDescription",
        "Mirage Hub native Rust daemon and command line",
    );
    resource.set("ProductName", "Mirage Hub");
    resource.set("LegalCopyright", "GPL-3.0-only; upstream notices retained");
    resource.set_manifest(r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
<trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges><requestedExecutionLevel level="asInvoker" uiAccess="false"/></requestedPrivileges></security></trustInfo>
<compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1"><application><supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/></application></compatibility>
</assembly>"#);
    resource
        .compile()
        .expect("native daemon resources must be embedded");
}
