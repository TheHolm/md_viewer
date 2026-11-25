//! Enforces a hard decision (see "Hard decisions" in `AGENTS.md`): the
//! viewer never fetches anything from the network, and the code must not
//! contain the ability to. Never weaken these tests.

use std::path::Path;

/// Crates that can talk to the network (HTTP clients and servers, TLS,
/// WebSockets); none may appear anywhere in the dependency tree.
const NETWORK_CRATES: &[&str] = &[
    "reqwest",
    "ureq",
    "hyper",
    "curl",
    "curl-sys",
    "isahc",
    "attohttpc",
    "surf",
    "h2",
    "h3",
    "quinn",
    "rustls",
    "native-tls",
    "openssl",
    "openssl-sys",
    "tungstenite",
    "tokio-tungstenite",
    "socket2",
    "mio",
];

/// Network APIs the source must not use: std sockets, GIO sockets and
/// remote-capable GIO URI files.
const NETWORK_APIS: &[&str] = &[
    "std::net",
    "TcpStream",
    "TcpListener",
    "UdpSocket",
    "SocketClient",
    "SocketConnection",
    "for_uri(",
    "for_commandline_arg",
    "gio::Resolver",
    "NetworkMonitor",
];

/// The repository root.
fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The package names listed in `Cargo.lock`.
fn locked_packages() -> Vec<String> {
    let lock = std::fs::read_to_string(root().join("Cargo.lock")).expect("read Cargo.lock");
    lock.lines()
        .filter_map(|line| line.strip_prefix("name = \""))
        .map(|name| name.trim_end_matches('"').to_string())
        .collect()
}

/// No network-capable crate is a dependency, directly or indirectly.
#[test]
fn no_network_crates() {
    let packages = locked_packages();
    assert!(packages.iter().any(|name| name == "md_viewer"));
    let found: Vec<&String> = packages
        .iter()
        .filter(|name| NETWORK_CRATES.contains(&name.as_str()))
        .collect();
    assert!(found.is_empty(), "network crates in Cargo.lock: {found:?}");
}

/// The source uses no network API.
#[test]
fn no_network_apis_in_source() {
    let mut checked = 0;
    for entry in std::fs::read_dir(root().join("src")).expect("read src") {
        let path = entry.expect("directory entry").path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read source");
        for api in NETWORK_APIS {
            assert!(!source.contains(api), "{} uses {api}", path.display());
        }
        checked += 1;
    }
    assert!(checked >= 5, "only {checked} source files checked");
}
