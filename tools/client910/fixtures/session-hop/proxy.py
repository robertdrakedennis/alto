#!/usr/bin/env python3
"""TCP relay for the world-hop run (run.sh).

    proxy.py LISTEN_PORT TARGET_PORT DROP_FILE

Forwards 127.0.0.1:LISTEN_PORT to 127.0.0.1:TARGET_PORT. While DROP_FILE
exists (run.sh touches it), every open client connection is cut on the client's
side only: the client sees its connection fail, while the server's side stays
open, like a link that dies without the server noticing. The file is removed
once the connections are cut. Stop it with SIGTERM.
"""
import os
import socket
import sys
import threading
import time

listen_port, target_port, drop_file = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
connections = []
lock = threading.Lock()


def pump(source, sink, drain):
    try:
        while True:
            data = source.recv(65536)
            if not data:
                break
            if not drain.is_set():
                sink.sendall(data)
    except OSError:
        pass
    # A side that closed normally closes the other (a cut connection does not:
    # the server's side stays open).
    if not drain.is_set():
        try:
            sink.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        sink.close()


def serve(client):
    upstream = socket.create_connection(("127.0.0.1", target_port))
    severed = threading.Event()
    with lock:
        connections.append((client, severed))
    threading.Thread(target=pump, args=(client, upstream, severed), daemon=True).start()
    # After the cut the server's replies are read and dropped so its side stays open.
    threading.Thread(target=pump, args=(upstream, client, severed), daemon=True).start()


def watch_drop_file():
    while True:
        if os.path.exists(drop_file):
            with lock:
                for client, severed in connections:
                    severed.set()
                    try:
                        client.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
                    client.close()
                connections.clear()
            os.remove(drop_file)
        time.sleep(0.05)


listener = socket.socket()
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("127.0.0.1", listen_port))
listener.listen()
threading.Thread(target=watch_drop_file, daemon=True).start()
while True:
    connection, _ = listener.accept()
    connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    serve(connection)
