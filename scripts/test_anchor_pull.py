"""
测试 Quest AnchorService 响应器（Pull 模式）
发一个查询包到 Quest:9945，等待回复并打印结果。
"""
import socket
import json
import sys

QUEST_IP  = sys.argv[1] if len(sys.argv) > 1 else "192.168.2.171"
QUEST_PORT = 9945
LOCAL_PORT = 9946   # 用 9946 监听回复，避免和 UE 的 9945 冲突

print(f"Sending query to {QUEST_IP}:{QUEST_PORT}")
print(f"Listening for response on local port {LOCAL_PORT}")
print()

s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.bind(("0.0.0.0", LOCAL_PORT))
s.settimeout(5.0)

# 发出查询包
s.sendto(b"?", (QUEST_IP, QUEST_PORT))
print(f"Query sent.")

try:
    data, addr = s.recvfrom(4096)
    print(f"=== Response from {addr} ===")
    try:
        parsed = json.loads(data)
        print(json.dumps(parsed, indent=2, ensure_ascii=False))
        status = parsed.get("status", "unknown")
        if status == "ready":
            print("\n✅  Anchor is ready — responder working correctly.")
        elif status == "not_found":
            print("\n⚠️  Anchor not found yet (T2 not implemented) — but responder IS working.")
        else:
            print(f"\n❓  Unknown status: {status}")
    except json.JSONDecodeError:
        print(f"Raw bytes: {data}")
except socket.timeout:
    print("❌  Timeout — no response from Quest after 5 seconds.")
    print("   Possible causes:")
    print("   1. App not running on Quest (check headset)")
    print("   2. Port 9945 blocked by firewall on Quest or network")
    print("   3. anchor_service responder failed to bind (check logcat)")
finally:
    s.close()
