# Test fixtures

`oidc-test-key.pem` and `oidc-other-key.pem` are RSA keys made for the
sign-in tests (tests/sign_in.rs) with `openssl genrsa -traditional 2048`.
They sign nothing but test ID tokens for a provider that exists only in
those tests. They are not secrets and protect nothing.
