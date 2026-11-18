# POP's release keys: the public halves

`ml-dsa-65.pub` and `ed25519.pub` go here, as OpenSSL writes them
(`openssl pkey -pubout`). Every release's SHA256SUMS is signed with both
private halves, and the updater, `tools/sign-release.sh` and anyone checking
a release by hand check against these. docs/releases.md says how they are
made and where the private halves live (the repository's secrets, and
nowhere else).

Until they are committed, the release workflow refuses to publish.
