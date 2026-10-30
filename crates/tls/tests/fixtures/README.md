# Disposable TLS test keys

All `*-key.der` files here are deliberately public test-only PKCS#8 private keys.
They authenticate only fixture certificates chained to `ca.der`; that authority
is never part of the default trust store. Never use these keys outside tests.

Generate a fresh set with `python3 tools/tls_fixtures.py /tmp/rustos-certificates`,
then copy `ca.der`, `{valid,expired,future,rsa}.der` and their matching
`{valid,expired,future,rsa}-key.der` files into this directory together.
The host suite uses fixed Unix time 1800000000 (2027); leaf validity dates are
explicitly selected to test current, expired and not-yet-valid certificates.
QEMU fixtures generate a fresh disposable authority/keys on every invocation.
