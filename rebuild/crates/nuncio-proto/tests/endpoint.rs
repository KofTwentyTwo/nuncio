use nuncio_proto::client::loopback_endpoint;

#[test]
fn loopback_transport_never_resolves_or_connects_to_remote_authorities() {
    for value in ["http://127.0.0.1:9421", "http://[::1]:9421"] {
        assert!(loopback_endpoint(value).is_ok());
    }
    for value in [
        "http://example.invalid",
        "http://192.0.2.1",
        "http://0.0.0.0:9421",
        "http://user:secret@127.0.0.1",
        "http://127.0.0.1/path",
        "http://127.0.0.1/?token=secret",
        "https://127.0.0.1",
    ] {
        assert!(loopback_endpoint(value).is_err());
    }
}
