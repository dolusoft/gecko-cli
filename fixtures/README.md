# Fixtures

`command-console-<sürüm>/` klasörleri command-console sürümünün yayın ekinden (`remote-fixtures-<sürüm>.tar.gz`) **olduğu gibi** kopyalanır: `remote/` örnekleri ve `remote-contract.md` sözleşmesi. Elle düzenlenmez, biçimlendirilmez (`.prettierignore`). Yeni sürüm yeni klasörle gelir.

| Klasör                   | Kaynak                                                    | Arşivin SHA-256                                                    |
| ------------------------ | --------------------------------------------------------- | ------------------------------------------------------------------ |
| `command-console-1.7.0/` | `dolusoft/command-console` `v1.7.0` (`2221f50`) yayın eki | `86def1d6ee1634209df1ec9e46d523994662f3f5ccf80c4aac041bf34b398274` |

Rust tarafında `crates/gecko-wire/tests/fixtures.rs` `valid`, `invalid`, `future`, `error-class.json` ve `command-name.json` örneklerini CLI'nın okuyucusuna karşı koşar. `sequence/` örnekleri satır yolunun süreç testleridir (adım 4b-5).
