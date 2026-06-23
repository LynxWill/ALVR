"""
发一次查询，持续监听 2 秒，统计 Quest 实际回复了几个包。
用于确认"返回两次"是 Quest 端重复发送，还是 UE socket 特有问题。
"""
import socket, json, sys, time

QUEST_IP = sys.argv[1] if len(sys.argv) > 1 else "192.168.2.171"
PORT = 9945

s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("0.0.0.0", PORT))

print(f"Bound 0.0.0.0:{PORT}, sending ONE query to {QUEST_IP}:{PORT}")
s.sendto(b"?", (QUEST_IP, PORT))

# 持续监听 2 秒，收集所有回包
s.settimeout(2.0)
count = 0
start = time.time()
while time.time() - start < 2.0:
    try:
        data, addr = s.recvfrom(4096)
        count += 1
        elapsed_ms = (time.time() - start) * 1000
        print(f"  [{count}] +{elapsed_ms:6.1f}ms  from {addr}: {data.decode(errors='replace')}")
    except socket.timeout:
        break

s.close()
print(f"\n=== Total responses received: {count} ===")
if count == 1:
    print("✅  Single response — duplication is NOT from Quest. (UE-side issue)")
elif count >= 2:
    print("⚠️  Quest/WiFi delivered duplicate packets — UE-side dedup (bPendingRequest) is the correct fix.")
else:
    print("❌  No response — Quest unreachable or app not running.")
