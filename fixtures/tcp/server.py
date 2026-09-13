import os
import socket

with socket.socket() as listener:
    listener.bind(("127.0.0.1", int(os.environ["PORT"])))
    listener.listen()
    while True:
        connection, _ = listener.accept()
        with connection:
            connection.settimeout(5)
            try:
                data = connection.recv(1024)
                if data:
                    connection.sendall(data)
            except (OSError, TimeoutError):
                pass
