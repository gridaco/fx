"""Content digest used by this example's medium contracts."""

import hashlib


def content_sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()
