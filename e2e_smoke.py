"""win-timer 端到端冒烟测试：创建 -> 列表 -> 禁用 -> 启用 -> 运行 -> 删除。

仅使用标准库，无需额外依赖。
"""
import json
import urllib.request
import urllib.error
from datetime import datetime, timedelta

BASE = "http://127.0.0.1:58081/api"
NAME = "win-timer-selftest"
results = []


def call(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        BASE + path,
        data=data,
        method=method,
        headers={"Content-Type": "application/json"} if data else {},
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, json.loads(r.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        raw = e.read().decode("utf-8", "replace")
        try:
            return e.code, json.loads(raw)
        except json.JSONDecodeError:
            return e.code, {"raw": raw}


def check(label, ok, detail=""):
    results.append((label, ok, detail))
    print(f"[{'PASS' if ok else 'FAIL'}] {label}" + (f" -- {detail}" if detail else ""))


def find(tasks):
    return next((t for t in tasks if t["name"] == NAME), None)


def main():
    # 0) health
    st, body = call("GET", "/health")
    check("health 返回 ok", st == 200 and body.get("status") == "ok", str(body))

    # 1) 创建（一次性，5 分钟后）
    at = (datetime.now() + timedelta(minutes=5)).strftime("%Y-%m-%dT%H:%M")
    st, body = call("POST", "/tasks", {
        "name": NAME,
        "program": r"C:\Windows\System32\cmd.exe",
        "arguments": "/c exit 0",
        "description": "win-timer 自测任务，可删除",
        "schedule": {"ty": "once", "at": at, "every_minutes": None},
    })
    check("创建任务", st == 200 and body.get("ok") is True, f"HTTP {st} {body}")
    if st != 200:
        return

    try:
        # 2) 列表里能看到
        st, body = call("GET", "/tasks?scope=root")
        t = find(body.get("tasks", []))
        check("列表包含新任务", t is not None, t["name"] if t else "未找到")
        check("程序路径正确",
              t and t["executable"] == r"C:\Windows\System32\cmd.exe",
              t["executable"] if t else "")
        check("参数正确", t and t["arguments"] == "/c exit 0", t["arguments"] if t else "")
        check("下次运行时间已设置", t and t["next_run_time"] is not None,
              t["next_run_time"] if t else "")

        # 3) 禁用
        st, body = call("POST", f"/tasks/disable/{NAME}")
        check("禁用任务", st == 200, f"HTTP {st} {body}")
        _, body = call("GET", "/tasks?scope=root")
        t = find(body.get("tasks", []))
        check("禁用后 enabled=false", t and t["enabled"] is False,
              f"enabled={t['enabled'] if t else '?'} state={t['state'] if t else '?'}")

        # 4) 启用
        st, body = call("POST", f"/tasks/enable/{NAME}")
        check("启用任务", st == 200, f"HTTP {st} {body}")
        _, body = call("GET", "/tasks?scope=root")
        t = find(body.get("tasks", []))
        check("启用后 enabled=true", t and t["enabled"] is True,
              f"enabled={t['enabled'] if t else '?'} state={t['state'] if t else '?'}")

        # 5) 运行
        st, body = call("POST", f"/tasks/run/{NAME}")
        check("运行任务", st == 200, f"HTTP {st} {body}")
    finally:
        # 6) 删除（无论上面成败都必须清理）
        st, body = call("DELETE", f"/tasks/{NAME}")
        check("删除任务", st == 200, f"HTTP {st} {body}")
        _, body = call("GET", "/tasks?scope=root")
        check("删除后列表中消失", find(body.get("tasks", [])) is None)

    # 7) 错误处理：未知操作 / 不存在的任务
    st, body = call("POST", f"/tasks/nuke/{NAME}")
    check("未知操作返回结构化错误", st == 500 and "error" in body, f"HTTP {st} {body}")

    # 7.5) 执行历史：接口可用且结构正确（历史记录未启用时允许空列表）
    st, body = call("GET", "/history")
    ok = st == 200 and "history_enabled" in body and "rows" in body
    check("执行历史接口返回结构正确", ok, f"HTTP {st} {str(body)[:120]}")

    st, body = call("GET", "/history?task=win-timer-selftest-nonexistent")
    ok = st == 200 and body.get("rows") == []
    check("执行历史按任务名过滤", ok, f"HTTP {st} {str(body)[:120]}")

    # 8) interval / daily 两条调度路径也要能落库
    for ty, extra in (("interval", {"every_minutes": 10}),
                      ("daily", {"at": (datetime.now() + timedelta(days=1)).strftime("%Y-%m-%dT%H:%M")})):
        st, body = call("POST", "/tasks", {
            "name": NAME,
            "program": r"C:\Windows\System32\cmd.exe",
            "arguments": "/c exit 0",
            "description": f"win-timer 自测 {ty}",
            "schedule": {"ty": ty, "every_minutes": extra.get("every_minutes"),
                         "at": extra.get("at")},
        })
        ok = st == 200 and body.get("ok") is True
        check(f"创建 {ty} 任务", ok, f"HTTP {st} {body}")
        if ok:
            call("DELETE", f"/tasks/{NAME}")

    st, body = call("POST", "/tasks", {
        "name": "bad", "program": "cmd.exe",
        "schedule": {"ty": "once", "at": None, "every_minutes": None},
    })
    check("缺少开始时间返回错误", st == 500 and "error" in body, f"HTTP {st} {body}")


if __name__ == "__main__":
    try:
        main()
    finally:
        failed = [r for r in results if not r[1]]
        print("\n" + "=" * 46)
        print(f"总计 {len(results)} 项，通过 {len(results) - len(failed)}，失败 {len(failed)}")
        for label, _, detail in failed:
            print(f"  FAILED: {label} -- {detail}")
