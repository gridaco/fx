"""``python -m grida.fx <verb> …``: runs the ``grida-fx`` binary (``grida.fx._api.binary()``) with
the same arguments and exits with its status, so Python users never need Node
(``docs/guide/05-running.md``).

The binary gets this process's environment, with ``GRIDA_FX_SDK_PYTHON`` set to this interpreter
(``grida.fx._api.engine_environment``): node bodies run in it when neither ``GRIDA_FX_PYTHON`` nor
a project ``.venv`` chooses a Python, so they run where ``grida`` is installed.

On POSIX the process becomes the binary (``os.execve``): signals, the terminal and the exit status
are the engine's own. Elsewhere the binary runs as a child and its exit status is returned; a
Ctrl-C reaches the child, which stops the run, and this process waits for it. No binary found,
or one that cannot be started: ``grida-fx: <reason>`` on stderr, exit status 2, as the engine
reports an error.
"""

from __future__ import annotations

import os
import subprocess
import sys

from grida.fx._api import binary, engine_environment


def main() -> int:
    arguments = sys.argv[1:]
    try:
        program = binary()
    except RuntimeError as error:
        print(f"grida-fx: {error}", file=sys.stderr)
        return 2
    environment = engine_environment()
    try:
        if os.name == "posix":
            sys.stdout.flush()
            sys.stderr.flush()
            os.execve(program, ["grida-fx", *arguments], environment)
        child = subprocess.Popen([str(program), *arguments], env=environment)
    except OSError as error:
        print(f"grida-fx: cannot start {program}: {error.strerror or error}", file=sys.stderr)
        return 2
    while True:
        try:
            return child.wait()
        except KeyboardInterrupt:
            continue


if __name__ == "__main__":
    raise SystemExit(main())
