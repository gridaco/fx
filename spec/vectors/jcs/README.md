# JCS vectors

`cases.json` (`"kind": "fx-jcs-vectors-v1"`) holds the canonical JSON cases that every implementation of [identity.md](../../identity.md) §1 and §2 must pass. Each case has a `name` and an `input`, which is a JSON text.

- **`canonical`**: read `input` as an FX value and canonicalize it. The UTF-8 bytes of the result must equal the UTF-8 bytes of `canonical`.
- **`refuse`**: reading `input` must fail. The value names the rule that refuses it. An implementation must refuse, but it does not have to report the same name.

| `refuse` | Input |
|---|---|
| `non_finite_number` | the tokens `NaN`, `Infinity`, `-Infinity` |
| `integer_out_of_range` | an integer literal (no fraction, no exponent) that is not the canonical form of the number it reads as: reading would round it, or it is exact but written another way |
| `number_overflow` | a number literal that rounds to ±infinity |
| `lone_surrogate` | an escaped surrogate (D800 to DFFF) that is not half of a high-low pair |
| `duplicate_key` | two equal keys in one object, compared after unescaping |
| `invalid_json` | text that is not JSON ([RFC 8259](https://www.rfc-editor.org/rfc/rfc8259)), such as an unknown escape (`invalid_json_escape_before_non_ascii` escapes a character of two UTF-8 bytes, so a reader that reports positions must not split it) |

Notes:

- Cases named `rfc8785_*` are the examples of [RFC 8785](https://www.rfc-editor.org/rfc/rfc8785) and the rows of its Appendix B. An Appendix B input is the Python `repr` of the row's bit pattern.
- An integer literal must be the canonical form of the number it reads as, apart from the sign of zero (`-0` is 0), so that a reader always reads back what it writes. `9007199254740992`, `10000000000000000` and `1152921504606847000` are the canonical forms of the numbers they read as, so they are accepted; `9007199254740993` (which reads as 2^53) and `1152921504606846976` (2^60 written out, whose canonical form is `1152921504606847000`) are refused; FX's messages tell the two apart, a number beyond the integers binary64 holds exactly and an exact number not written canonically. No literal of more than 21 digits is canonical, so `integer_5000_digits` is refused by its length and must not crash a reader. A literal with a fraction or an exponent is a binary64 number, rounded as usual: `9007199254740993.0` reads as 2^53 and `1e-400` reads as 0.
- Every accepted case was checked against the Python `rfc8785` package and against a canonicalizer built on ECMAScript `JSON.stringify`.
