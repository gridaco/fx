# Grida provider v1 compatibility inputs

FX's synthetic fixtures cover Grida's provider credentials contract as of commit
`273961f8f050e4c9fd3df8714f1fe409484d5b00`. The Rust suite runs these locally without
a Grida installation or network access.

- [Grida provider credentials protocol](https://github.com/gridaco/grida/blob/273961f8f050e4c9fd3df8714f1fe409484d5b00/packages/grida-auth/PROVIDER-CREDENTIALS-V1.md)
- [Grida compatibility fixtures](https://github.com/gridaco/grida/tree/273961f8f050e4c9fd3df8714f1fe409484d5b00/packages/grida-auth/fixtures/providers-v1)

| File | Expected behavior |
|---|---|
| `empty.toml` | An empty store provides no key. |
| `escaping.toml` | The key preserves Unicode, escaped characters and surrounding spaces. |
| `tombstone.toml` | A removed provider provides no key before migration. |
| `pending.toml` | Reads report `migration_pending` while migration is pending. |
| `unknown-field.toml` | An unknown provider field yields `invalid_store`. |
| `unknown-version.toml` | An unknown version yields `unsupported_version`. |
