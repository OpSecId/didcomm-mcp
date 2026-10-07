//! Makes sure `ui/dist` exists, so the embedded web UI compiles before `npm run build`
//! has been run: a placeholder page says how to build it.

fn main() {
    let dist = std::path::Path::new("ui/dist");
    let index = dist.join("index.html");
    if !index.exists() {
        std::fs::create_dir_all(dist).expect("creating ui/dist");
        std::fs::write(
            &index,
            "<!doctype html><meta charset=utf-8><title>didcomm-mcp</title>\
             <p style=\"font-family:sans-serif\">The web UI isn't built. Run <code>npm ci &amp;&amp; npm run build</code> in <code>ui/</code>, then rebuild.</p>\n",
        )
        .expect("writing the placeholder ui/dist/index.html");
    }
    println!("cargo:rerun-if-changed=ui/dist");
    println!("cargo:rerun-if-changed=build.rs");
}
