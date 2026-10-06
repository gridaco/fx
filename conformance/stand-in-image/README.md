# stand-in-image

A stand-in run answers the paid calls the cache cannot answer with a function the caller names,
offline and for nothing ([protocol.md](../../spec/protocol.md) §5.7, §6.1 *Stand-in answers*;
[store.md](../../spec/store.md) §8 *Stand-in runs*). Here the function is `answer` in
`in/stand_in.py`, given as `--stand-in stand_in.py#answer`: the engine starts a stand-in host
(the Python host, so the case needs `GRIDA_FX_PYTHON` although it has no `nodes/`) in its working
directory, the project the case runs in, and loads the file there.

## What the stand-in writes

The other stand-in cases (`stand-in-errors`, `stand-in-agent`, `stand-in-job`,
`workflows-setting`) keep the same two notes, with the same code:

- `count.txt`: how many calls it was asked, over every invocation of the case, as one number and
  a line feed.
- `asked.json`: one entry per call it was asked, sorted by its JSON text so the file does not
  depend on the order calls arrive in: `capability`, `route` (`id`, `fingerprint`), `request` as
  the wire carries it (each file as `{"file": <digest>}`), `take` (the list), `key` (the call
  key), `instance` (`id`, `path`, `step`), and `files`, by digest, each with the `name`, `kind`,
  `size` and `facts` of its file ref and `"path_is_file": true` when the ref's `path` names a
  file it can read. The path itself is never written: it is a private absolute path.

The route `img-a@acme` declares no contract, so its fingerprint is the route example of
[identity.md](../../spec/identity.md) §14,
`4d41b81c56215efdd18574eab8e2b704a8ecc86af08d569f55cd29973c4e7ed4`, as in `cache-replay`.

## The picture

The stand-in answers with a 64×64 RGB PNG of one colour, `(40, 90, 160)`, which it makes when it
is asked: the signature, an `IHDR` chunk (bit depth 8, colour type 2, no interlace), one `IDAT`
chunk and an empty `IEND` chunk. The `IDAT` data is a zlib stream of stored (uncompressed)
deflate blocks that the stand-in writes itself rather than with `zlib.compress`, so the bytes,
and every digest made from them, are the same whatever zlib the Python that runs it was built
with. An RGB PNG has no alpha channel, so it is opaque, as `background: opaque` asks.

## Store isolation

The project's own store, `.fx/cache`, never gets a file from a stand-in run: the run's records
and files go to the stand-in store, `.fx/cache/stand-in`. The case checks that the project's
`files/`, `results/` and `calls/` hold no file after three stand-in runs, and that a run without
a stand-in misses the call the stand-in answered.

The workflow's `vars: {mood: …}` is never used by its prompt. `vars` is spent on rendering the
prompt and is not sent ([identity.md](../../spec/identity.md) §9), so `--mood stormy` gives
the step another identity, and a result-cache miss, while its call has the same key: that run's
call is a hit of the stand-in store's call cache.
