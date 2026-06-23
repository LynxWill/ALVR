"""
测试 PC:9945 能否收到 Quest 的回包（模拟 UE Socket 的角色）
- 绑定到 0.0.0.0:9945（和 UE 一样）
- 发查询到 Quest:9945
- 等待回复
"""
import socket, json, sys

QUEST_IP = sys.argv[1] if len(sys.argv) > 1 else "192.168.2.171"
PORT = 9945

s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("0.0.0.0", PORT))
s.settimeout(5.0)

print(f"Bound to 0.0.0.0:{PORT}  (same as UE plugin)")
print(f"Sending query to {QUEST_IP}:{PORT} ...")
s.sendto(b"?", (QUEST_IP, PORT))

try:
    data, addr = s.recvfrom(4096)
    print(f"✅  Received from {addr}:")
    print(json.dumps(json.loads(data), indent=2))
    print("\nPC:9945 can receive Quest responses — UE socket issue must be elsewhere.")
except socket.timeout:
    print("❌  Timeout on port 9945 — response did not arrive.")
    print("   Likely Windows Firewall is blocking inbound UDP 9945.")
finally:
    s.close()
