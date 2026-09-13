import os
import socket
import urllib.parse

url = urllib.parse.urlsplit(os.environ["ECHO"])
message = b"tcp-fixture"
with socket.create_connection((url.hostname, url.port), timeout=5) as connection:
    connection.sendall(message)
    received = bytearray()
    while len(received) < len(message):
        chunk = connection.recv(len(message) - len(received))
        if not chunk:
            raise RuntimeError("echo listener closed before returning the message")
        received.extend(chunk)
    if received != message:
        raise RuntimeError(f"unexpected echo: {received!r}")
print(os.environ["ECHO"])
