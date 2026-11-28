import sqlite3
import sys

import idna


def main():
    who = sys.argv[1] if len(sys.argv) > 1 else "world"
    python = "%d.%d" % sys.version_info[:2]
    # From PyPI (idna), and from Python's own C modules (sqlite3).
    domain = idna.encode("bücher.example").decode()
    sqlite = sqlite3.connect(":memory:").execute("select sqlite_version()").fetchone()[0]
    print(f"py-hello 1.2.0: hello, {who} (Python {python}, {domain}, sqlite {sqlite})")
    return 0
