import socket
import json
import time
import random

HOST = '127.0.0.1'
PORT = 9132
def main():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.connect((HOST, PORT))
        print(f"Connected to the game server at {HOST}:{PORT}")

        try:
            for _ in range(10): # Send 10 updates
                x = random.random() * 10 
                y = random.random() * 10 
                pos = (x, y)
                message = json.dumps(pos).encode('utf-8')
                print(f"Sending player position: {pos}")
                s.sendall(message)
                print(f"Sent player position: {pos}")
                # Receive the updated game state
                data = s.recv(1024)
                print("Received game state:", data.decode('utf-8'))

                time.sleep(1)
        except Exception as e:
            print(f"Error: {e}")
if __name__ == "__main__":
    main()

