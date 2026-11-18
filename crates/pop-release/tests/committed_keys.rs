//! The release public keys committed in packaging/release-keys read as the
//! two keys a release is checked with (#148): a wrong file, a swapped pair or
//! a private half committed by mistake fails here, not on release day.

#[test]
fn the_committed_release_keys_read() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/release-keys");
    let read = |f: &str| {
        std::fs::read_to_string(format!("{dir}/{f}"))
            .unwrap_or_else(|e| panic!("packaging/release-keys/{f}: {e}"))
    };
    let (ml, ed) = (read("ml-dsa-65.pub"), read("ed25519.pub"));
    for (name, text) in [("ml-dsa-65.pub", &ml), ("ed25519.pub", &ed)] {
        assert!(!text.contains("PRIVATE"), "{name} holds a private key");
    }
    pop_release::Keys::from_pem(&ml, &ed).unwrap();
}
