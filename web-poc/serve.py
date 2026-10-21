#!/usr/bin/env python3
"""Static server for the self_rust_os browser POC.

QEMU Wasm needs SharedArrayBuffer (multi-threaded TCG), which browsers
only expose to cross-origin isolated pages. This server sends the
required headers on every response:

    Cross-Origin-Opener-Policy: same-origin
    Cross-Origin-Embedder-Policy: require-corp

Usage: ./serve.py [port]     (default 8088, serves ./htdocs)
"""

import functools
import http.server
import mimetypes
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent / "htdocs"


class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cross-Origin-Resource-Policy", "same-origin")
        super().end_headers()


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8088
    mimetypes.add_type("application/wasm", ".wasm")
    handler = functools.partial(Handler, directory=str(ROOT))
    with http.server.ThreadingHTTPServer(("127.0.0.1", port), handler) as srv:
        print(f"Serving {ROOT} at http://127.0.0.1:{port}/ (COOP/COEP enabled)")
        srv.serve_forever()


if __name__ == "__main__":
    main()
