import socket
import json

s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.bind(("0.0.0.0", 9945))
print("Listening on UDP 9945... (waiting for anchor packet from Quest)")
print("Connect ALVR_Lynx on Quest to trigger the test packet.\n")

while True:
    data, addr = s.recvfrom(4096)
    try:
        parsed = json.loads(data)
        print(f"=== Received from {addr} ===")
        print(json.dumps(parsed, indent=2))
        print("=== T6 verification PASSED ===")
    except Exception as e:
        print(f"Raw ({addr}): {data}")
        print(f"Parse error: {e}")
