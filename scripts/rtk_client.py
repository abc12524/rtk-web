#!/usr/bin/env python3
"""rtk-web 客户端：把文本、文件或命令输出发送到压缩服务。

示例:
    printf 'a\\na\\na\\n' | python rtk_client.py text
    python rtk_client.py file build.log --stats
    python rtk_client.py run -- cargo test -- --nocapture
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid

DEFAULT_URL = os.environ.get("RTK_WEB_URL", "http://192.168.30.181:8787")

STATUS_HEADERS = (
    "x-rtk-format",
    "x-rtk-mode",
    "x-rtk-charset",
    "x-rtk-lossless",
    "x-rtk-lossy",
    "x-rtk-changed",
    "x-rtk-chain",
    "x-rtk-bytes-before",
    "x-rtk-bytes-after",
    "x-rtk-saved-percent",
    "x-rtk-tokens-before",
    "x-rtk-tokens-after",
)


def write_stdout(data: bytes) -> None:
    sys.stdout.buffer.write(data)


def write_stderr(text: str) -> None:
    sys.stderr.buffer.write(text.encode("utf-8", "replace"))
    sys.stderr.buffer.flush()


def build_query(args: argparse.Namespace, response: str | None = None) -> str:
    params = {
        "mode": args.mode,
        "lossless": "true" if args.lossless else "false",
        "charset": args.charset,
    }
    if args.format:
        params["format"] = args.format
    if response:
        params["response"] = response
    return urllib.parse.urlencode(params)


def post(url: str, data: bytes, headers: dict[str, str] | None, timeout: float):
    request = urllib.request.Request(url, data=data, headers=headers or {}, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.read(), response.headers
    except urllib.error.HTTPError as err:
        detail = err.read().decode("utf-8", "replace")
        raise SystemExit(f"rtk-web: HTTP {err.code}: {detail}") from err
    except urllib.error.URLError as err:
        raise SystemExit(f"rtk-web: cannot reach {url}: {err.reason}") from err


def post_text(base: str, data: bytes, args: argparse.Namespace):
    url = f"{base.rstrip('/')}/api/compress?{build_query(args)}"
    return post(url, data, None, args.timeout)


def post_file(base: str, path: str, args: argparse.Namespace):
    boundary = f"----rtkweb{uuid.uuid4().hex}"
    filename = os.path.basename(path).replace('"', "")
    try:
        with open(path, "rb") as handle:
            content = handle.read()
    except OSError as err:
        raise SystemExit(f"rtk-web: cannot read {path}: {err}") from err

    head = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'
        "Content-Type: application/octet-stream\r\n\r\n"
    ).encode("utf-8")
    tail = f"\r\n--{boundary}--\r\n".encode("ascii")
    body = head + content + tail

    response = "json" if args.json else None
    url = f"{base.rstrip('/')}/api/compress/file?{build_query(args, response)}"
    headers = {"Content-Type": f"multipart/form-data; boundary={boundary}"}
    return post(url, body, headers, args.timeout)


def print_stats(headers) -> None:
    lines = [
        f"{name}: {headers[name]}"
        for name in STATUS_HEADERS
        if headers.get(name) is not None
    ]
    if lines:
        write_stderr("\n".join(lines) + "\n")


def emit(body: bytes, headers, args: argparse.Namespace) -> None:
    if args.stats:
        print_stats(headers)

    if args.json:
        try:
            payload = json.loads(body)
        except json.JSONDecodeError:
            payload = {
                "compressed": body.decode("utf-8", "replace"),
                "stats": {name: headers.get(name) for name in STATUS_HEADERS},
            }
        write_stdout(
            json.dumps(payload, ensure_ascii=False, indent=2).encode("utf-8") + b"\n"
        )
        return

    write_stdout(body)
    if body and not body.endswith(b"\n"):
        write_stdout(b"\n")


def cmd_text(base: str, args: argparse.Namespace) -> int:
    data = sys.stdin.buffer.read()
    body, headers = post_text(base, data, args)
    emit(body, headers, args)
    return 0


def cmd_file(base: str, args: argparse.Namespace) -> int:
    body, headers = post_file(base, args.path, args)
    emit(body, headers, args)
    return 0


def cmd_run(base: str, args: argparse.Namespace) -> int:
    command = args.command
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        write_stderr("rtk-web: run requires a command\n")
        return 2

    completed = subprocess.run(command, capture_output=True)
    body, headers = post_text(base, completed.stdout + completed.stderr, args)
    emit(body, headers, args)
    return completed.returncode


def build_parser() -> argparse.ArgumentParser:
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--url", default=DEFAULT_URL, help="服务地址")
    common.add_argument(
        "--format",
        default=None,
        help="term / json / web / matches / lockfile / auto（文件模式默认按文件名推断）",
    )
    common.add_argument("--mode", choices=("chain", "auto"), default="chain")
    common.add_argument("--lossless", action="store_true", help="只做 ANSI 清理与重复行折叠")
    common.add_argument(
        "--charset",
        choices=("auto", "utf-8", "utf-16le", "utf-16be", "gbk"),
        default="auto",
    )
    common.add_argument("--json", action="store_true", help="输出结构化 JSON")
    common.add_argument("--stats", action="store_true", help="把压缩统计写到 stderr")
    common.add_argument("--timeout", type=float, default=60.0)

    parser = argparse.ArgumentParser(description="rtk-web 客户端")
    sub = parser.add_subparsers(dest="command", required=True)

    text = sub.add_parser("text", parents=[common], help="压缩 stdin")
    text.set_defaults(run=cmd_text)

    file_cmd = sub.add_parser("file", parents=[common], help="上传文件并压缩")
    file_cmd.add_argument("path", help="文本文件路径")
    file_cmd.set_defaults(run=cmd_file)

    run_cmd = sub.add_parser("run", parents=[common], help="执行命令并压缩其输出")
    run_cmd.add_argument("command", nargs=argparse.REMAINDER)
    run_cmd.set_defaults(run=cmd_run)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.run(args.url, args)


if __name__ == "__main__":
    raise SystemExit(main())