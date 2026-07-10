"""Small smoke client for Glebin's newline-delimited JSON protocol."""

import json
import socket

HOST = "127.0.0.1"
PORT = 9132


def send_message(writer, message):
    writer.write((json.dumps(message) + "\n").encode("utf-8"))
    writer.flush()


def receive_message(reader):
    line = reader.readline()
    if not line:
        raise ConnectionError("server closed the connection")
    return json.loads(line)


def main():
    with socket.create_connection((HOST, PORT), timeout=2) as connection:
        reader = connection.makefile("rb")
        writer = connection.makefile("wb")
        welcome = receive_message(reader)
        print("Welcome:", welcome)
        player_id = welcome["player_id"]

        send_message(writer, {"type": "set_name", "name": "python-smoke"})
        for dx, dy in [(1, 0), (0, 1), (-1, 0), (0, -1)]:
            send_message(writer, {"type": "move", "dx": dx, "dy": dy})

        while True:
            message = receive_message(reader)
            if (
                message.get("type") == "snapshot"
                and player_id in message["snapshot"]["players"]
            ):
                print("Snapshot:", message)
                break


if __name__ == "__main__":
    main()
