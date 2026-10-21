// A release executable with deliberately different embedded integrations.
use std::io::Read;
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t"))
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("__upgrade-render") => {
            let binary = quote(&args[2]);
            let pi = quote(&format!("// outrim-managed: pi-extension\nconst BRIDGE_EXECUTABLE = {binary};\n// new release fixture"));
            let opencode = quote(&format!("// outrim-managed: opencode-plugin\nconst BRIDGE_EXECUTABLE = {binary};\n// new release fixture"));
            println!("[{pi},{opencode}]");
        }
        Some("bridge") => { let _ = std::io::stdin().read_to_end(&mut Vec::new()); }
        _ => println!("outrim 999.0.0"),
    }
}
