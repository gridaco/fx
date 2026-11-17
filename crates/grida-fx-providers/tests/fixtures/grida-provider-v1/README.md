# Grida provider v1 compatibility inputs

These inputs are byte-for-byte copies of Grida's synthetic, credential-free protocol
fixtures at commit `273961f8f050e4c9fd3df8714f1fe409484d5b00`. Grida's fixture documentation explicitly permits
ports to reuse them without a Node runtime. They are maintained here so ordinary FX
checks need no network, Grida installation, or sibling checkout.

- [Normative protocol](https://github.com/gridaco/grida/blob/273961f8f050e4c9fd3df8714f1fe409484d5b00/packages/grida-auth/PROVIDER-CREDENTIALS-V1.md)
- [Upstream inputs and expected results](https://github.com/gridaco/grida/tree/273961f8f050e4c9fd3df8714f1fe409484d5b00/packages/grida-auth/fixtures/providers-v1)
- [Upstream Apache-2.0 license](https://github.com/gridaco/grida/blob/273961f8f050e4c9fd3df8714f1fe409484d5b00/LICENSE)

Preserve these original inputs when updating the reference. Record the reviewed SHA
and checksums together; add FX-specific malformed and filesystem cases in the Rust
tests, using synthetic literals. No fixture contains a real provider key.

| File | SHA-256 |
|---|---|
| `empty.toml` | `da0ec13e5391f06cb14d5e40a4345cdd31c322248c30fe8fe0e100ee4a86f059` |
| `escaping.toml` | `bc03b7df7838f6ae2ac0a20a66154313dd54318461feb8aec3259d8fccd9c026` |
| `tombstone.toml` | `458e1944b871bb110be52eba16d8093002a42d75175ba1213ac5c67ff15c0f93` |
| `pending.toml` | `8bf84852c1a398bb321f7cf00393edd3915ddca2def0e6d36d909c56ef5e5ce2` |
| `unknown-version.toml` | `09b871d8b5c3a5c1e7eb9dff991fd28f13219457ef283d6dc2bec462ce680295` |
| `unknown-field.toml` | `0bd4088003dc697ec9a94e197c5ea4933416dd663a6efe5cfccb2333efca5b7b` |
