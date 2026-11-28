# A web server as slow as a slow connection, for tests/app-bundle.sh's delta
# stage: every response together gets at most RATE bytes a second.
# usage: python3 slow-httpd.py <dir> <port> <bytes a second>
import functools
import http.server
import sys
import threading
import time

root, port, rate = sys.argv[1], int(sys.argv[2]), float(sys.argv[3])
lock = threading.Lock()
next_free = [time.monotonic()]


def wait_for(n):
    # One link shared by every download: each piece waits its turn.
    with lock:
        now = time.monotonic()
        start = max(now, next_free[0])
        next_free[0] = start + n / rate
    time.sleep(max(0.0, start + n / rate - now))


class Slow(http.server.SimpleHTTPRequestHandler):
    def copyfile(self, source, dest):
        while True:
            chunk = source.read(16384)
            if not chunk:
                break
            wait_for(len(chunk))
            try:
                dest.write(chunk)
            except (BrokenPipeError, ConnectionResetError):
                break

    def log_message(self, *args):
        pass


http.server.ThreadingHTTPServer.daemon_threads = True
http.server.ThreadingHTTPServer(("127.0.0.1", port), functools.partial(Slow, directory=root)).serve_forever()
