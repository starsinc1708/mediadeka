use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::OnceLock,
};

static ENDPOINT: OnceLock<SocketAddrV4> = OnceLock::new();

pub(crate) fn use_loopback(address: SocketAddr) {
    let SocketAddr::V4(address) = address else {
        panic!("expected an IPv4 loopback fixture");
    };
    assert_eq!(*address.ip(), Ipv4Addr::LOCALHOST);
    // The fixture runs in its own test process.
    ENDPOINT
        .set(address)
        .expect("single Yandex fixture endpoint");
}

pub(super) fn endpoint(path: &str) -> Option<String> {
    ENDPOINT
        .get()
        .map(|address| format!("http://{address}{path}"))
}

pub(super) fn accepts_stream(url: &url::Url) -> bool {
    ENDPOINT.get().is_some_and(|address| {
        url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.port() == Some(address.port())
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    })
}
