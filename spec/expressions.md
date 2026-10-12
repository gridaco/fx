# Expressions

Workflow values hold expressions, `${{ … }}`. The guide ([02-workflow-file.md](../docs/guide/02-workflow-file.md#expressions)) describes the language; this document is normative for the rules written here, and grows as more of the language is pinned down. Values are those of [identity.md](identity.md) §1, with the engine's runtime values (missing, failed, pending, files, collections) as [identity.md](identity.md) §3 lists them; text renders as [identity.md](identity.md) §5 says.

The words MUST, MUST NOT and SHOULD are used as in RFC 2119.

## 1. Nothing

A value is *nothing* when it is `null` or missing. Missing is what a skipped or absent step gives, and what any member or index of a missing value gives (`steps.skipped.outputs.image.width` is missing). A failed value is not nothing.

## 2. The fallback, `??`

`a ?? b` is `a` when `a` is not nothing, and `b` when it is. `b` is evaluated only when `a` is nothing: otherwise it records no reads and raises no error. `??` is right-associative, so `a ?? b ?? c` is `a ?? (b ?? c)`, and has the lowest precedence of every operator: `a ?? b == c` is `a ?? (b == c)`.

When `a` is pending (its value is known only once a step runs), `b` is evaluated and the result is pending on both.

**A missing field.** When the left operand of `??` is itself a field access, `x.name` or `x['name']` with a string index, and `x` is an object that has no member `name`, the access gives missing instead of failing, so `??` gives its right operand. An object here is an object value or a file whose JSON content is an object; `facts(f)` and an object in a table are objects. For example, over `tables: { rows: [ { id: a, note: first }, { id: b } ] }`, `item.note ?? 'none'` is `first` for the first row and `none` for the second.

The rule is narrow on purpose, so a misspelt name elsewhere still fails:

- **Only the last access.** In `x.p.q ?? b`, an object `x.p` without `q` falls back; an object `x` without `p` fails with `no field 'p'`, as it would without `??`.
- **Only the operand itself.** The access must be the whole left operand; parentheses around it do not change that. `len(x.name) ?? b`, `!x.name ?? b` and `(x.name == 'a') ?? b` fail on a missing `name`, and so does a missing field in `??`'s right operand, except one that is the left operand of a further `??`: in `x.p ?? y.q ?? b`, both a missing `p` and a missing `q` fall back.
- **Not a declared name.** `inputs.name`, `tables.name` and a step's `outputs.name` (`steps.draw.outputs.name`, in a group or a used workflow too) fail on a missing `name` even under `??`: the workflow declares every input and table, and a step's type every output, so a missing one is a misspelling. An input not given is null, so `inputs.size ?? 'auto'` falls back on an input left out; `inputs.sise ?? 'auto'` fails with `no field 'sise'`. Fields inside their values (a table's rows, an input object's members, a JSON output's content) and a step's `facts`, which are what its body reported, follow the rule above: `steps.review.facts.verdict ?? 'accept'` falls back.
- **Only a missing member of an object.** Everything else an access refuses is still refused: a field of a value that is not an object (`a list has no field 'name'`), an object indexed by a non-string (`no entry 1`), a list index outside the list, a member of a file whose content is not a JSON object (`a file has no field 'width'; …`), and a name a step scope does not know (`let.typo`, `steps.typo`, an unknown root name).
- **Everything else is unchanged.** A member of a missing value is missing and of a failed value is failed, as anywhere. A pending `x` makes the access pending, and `??` waits for it as above.

An implementation MUST NOT record a read of the absent member as a source of the result: the result is the right operand's ([store.md](store.md), named connection metadata).

This rule only turns errors into values. An expression that evaluated before it gives the same value, so no identity changes ([identity.md](identity.md)).
