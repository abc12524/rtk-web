# rtk-web

把 bash / PowerShell 的原始输出文本交给 rtk 压缩引擎，返回压缩后的内容。服务不执行任何命令，只处理文本。

## 依赖结构

rtk 的源码已内嵌在 `vendor/rtk`，仓库自包含，克隆后可直接构建，不再需要同级 `rtk` 目录。

rtk 上游仓库：[https://github.com/rtk-ai/rtk](https://github.com/rtk-ai/rtk)

```text
rtk-web/
├── vendor/rtk/     # rtk CLI、rtk-engine、rtk-text、plugin
├── src/
└── Cargo.toml
```

`Cargo.toml` 使用：

```toml
rtk = { path = "vendor/rtk" }
rtk-engine = { path = "vendor/rtk/engine" }
```

升级 rtk 时，用上游源码覆盖 `vendor/rtk`，再重新应用本仓库对 `src/filter_api.rs`、`src/cmds/system/ls.rs` 等文件的改动。

> `rtk-engine` 的许可证是 **BUSL-1.1**，`vendor/rtk/LICENSE` 是 Apache-2.0。内部自用生产免费；对外提供托管服务需要先获得 rtk-ai 授权。

## 运行

```bash
cargo run
# 监听 http://127.0.0.1:8787
```

环境变量：

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `RTK_WEB_ADDR` | `127.0.0.1:8787` | 监听地址 |
| `RTK_WEB_MAX_BODY` | `10485760`（10 MiB） | 请求体上限 |
| `RTK_WEB_MAX_CONCURRENCY` | `8` | 同时执行压缩的任务数 |
| `RTK_WEB_TIMEOUT_MS` | `30000` | 单次压缩超时 |
| `RUST_LOG` | `rtk_web=info` | 日志级别 |

## API

### GET /health

```json
{"status":"ok","service":"rtk-web","version":"0.1.1"}
```

### GET /api/formats

```json
["term","json","web","matches","lockfile","auto"]
```

### GET /api/filters

返回已注册的命令专属过滤器名称。

```json
["black","cargo","cargo-build","cargo-clippy","ls","pytest","tsc", "..."]
```

### POST /api/compress

请求体是原始文本，响应体是压缩后的文本。统计信息放在响应头。

```bash
curl -X POST \
  --data-binary @output.txt \
  "http://127.0.0.1:8787/api/compress?format=term&mode=chain&lossless=false&charset=auto"
```

| 参数 | 默认值 | 说明 |
| --- | --- | --- |
| `format` | `term` | `term` / `json` / `web` / `matches` / `lockfile` / `auto` |
| `mode` | `chain` | `chain`（固定链，可预期）/ `auto`（形状感知，激进有损） |
| `lossless` | `false` | `true` 时只执行 `ansi + dedup`，不截断 |
| `charset` | `auto` | `auto` / `utf-8` / `utf-16le` / `utf-16be` / `gbk` |
| `filter` | 无 | 命令专属过滤器名称，见 `GET /api/filters`；优先于 `format` / `mode` / `lossless` |

指定 `filter` 时，服务调用 `src/cmds` 中对应的命令专属解析器，而不是通用链：

```bash
# ls -l（支持中英文 locale）
ls -l | curl -X POST --data-binary @- \
  "http://127.0.0.1:8787/api/compress?filter=ls"

# cargo test
cargo test 2>&1 | curl -X POST --data-binary @- \
  "http://127.0.0.1:8787/api/compress?filter=cargo-test"
```

`filter` 的输入必须是对应命令的原始输出。部分过滤器要求特定格式，例如 `git-status` 需要 `git status --porcelain -b`，`go-test` 需要 `go test -json`；格式不匹配时过滤器通常会回退原文。

响应头：

```text
X-Rtk-Format: term
X-Rtk-Mode: chain
X-Rtk-Charset: auto
X-Rtk-Lossless: false
X-Rtk-Lossy: true
X-Rtk-Changed: true
X-Rtk-Chain: ansi>progress>dedup>base64-elide>cap
X-Rtk-Bytes-Before: 11424
X-Rtk-Bytes-After: 6803
X-Rtk-Saved-Percent: 40.4
X-Rtk-Tokens-Before: 2856
X-Rtk-Tokens-After: 1701
```

### POST /api/compress/json

请求与响应都是 JSON，适合服务间调用。

```bash
curl -X POST http://127.0.0.1:8787/api/compress/json \
  -H "Content-Type: application/json" \
  -d '{"text":"a\na\na\na\n","format":"term","mode":"chain","lossless":false,"charset":"auto"}'
```

```json
{
  "compressed": "a\n[x4]\n",
  "format": "term",
  "mode": "chain",
  "lossless": false,
  "chain": ["ansi", "progress", "dedup", "base64-elide", "cap"],
  "lossy": true,
  "changed": true,
  "stats": {
    "bytes_before": 8,
    "bytes_after": 7,
    "bytes_saved": 1,
    "saved_percent": 12.5,
    "tokens_before": 2,
    "tokens_after": 2
  }
}
```

### POST /api/compress/file

上传文本文件，服务端读取内容后压缩。`multipart/form-data` 的字段名必须是 `file`。

```bash
curl -X POST http://127.0.0.1:8787/api/compress/file \
  -F "file=@build.log"
```

默认按文件名推断格式：

| 文件名 | 格式 |
| --- | --- |
| `*.json` | `json` |
| `*.html` / `*.htm` | `web` |
| `Cargo.lock` / `*.lock` | `lockfile` |
| 其他（`*.log`、`*.txt`） | `term` |

查询参数与 `/api/compress` 相同，额外支持 `response`：

| 参数 | 默认值 | 说明 |
| --- | --- | --- |
| `format` | 按文件名推断 | `term` / `json` / `web` / `matches` / `lockfile` / `auto` |
| `mode` | `chain` | `chain` 或 `auto` |
| `lossless` | `false` | `true` 时只执行 `ansi + dedup` |
| `charset` | `auto` | `auto` / `utf-8` / `utf-16le` / `utf-16be` / `gbk` |
| `response` | `text` | `text` 返回压缩文本；`json` 返回结构化统计，含原文件名 |

```bash
# 返回文本
curl -X POST "http://127.0.0.1:8787/api/compress/file?response=text" \
  -F "file=@build.log"

# 返回 JSON（包含 filename）
curl -X POST "http://127.0.0.1:8787/api/compress/file?response=json" \
  -F "file=@data.json"

# 覆盖文件名推断 + 无损模式
curl -X POST "http://127.0.0.1:8787/api/compress/file?format=term&lossless=true" \
  -F "file=@output.txt"
```

限制：

- 只接受文本文件，含 NUL 字节的二进制文件返回 `400`（带 UTF-16 BOM 的文件除外）。
- 请求体上限仍为 `RTK_WEB_MAX_BODY`，multipart 边界也计入。

## 错误响应

统一为 JSON：

```json
{"error":"unsupported mode `turbo` (use chain or auto)"}
```

| 状态码 | 含义 |
| --- | --- |
| `400` | 参数非法、编码无法解码、JSON 解析失败 |
| `413` | 请求体超过 `RTK_WEB_MAX_BODY` |
| `500` | 压缩任务失败 |
| `504` | 压缩超时 |

## 压缩语义

- `chain` 模式：按格式选择引擎内置链。`term` 链为 `ansi → progress → dedup → base64-elide → cap`。
- `auto` 模式：形状感知策略，压缩率更高，但会明显丢内容，仅建议显式开启。
- `lossless=true`：只运行 `ansi` 与 `dedup`。`dedup` 会把连续重复行折叠为 `[xN]`，属于语义无损。
- 引擎的 `never_worse` 保护保证输出不会比输入更大；方法 panic 时回退原文。

## Python 客户端

`scripts/rtk_client.py` 只依赖 Python 标准库。

```bash
# 压缩 stdin
printf 'a\na\na\n' | python scripts/rtk_client.py text

# 上传文件（按文件名推断格式）
python scripts/rtk_client.py file build.log

# 查看压缩统计（写到 stderr）
python scripts/rtk_client.py file build.log --stats

# 输出结构化 JSON
python scripts/rtk_client.py file build.log --json

# 执行命令并压缩它的 stdout + stderr
python scripts/rtk_client.py run -- cargo test -- --nocapture
```

| 参数 | 说明 |
| --- | --- |
| `--url` | 服务地址，默认 `http://192.168.30.181:8787`，也可用 `RTK_WEB_URL` 环境变量 |
| `--format` | `term` / `json` / `web` / `matches` / `lockfile` / `auto` |
| `--mode` | `chain`（默认）或 `auto` |
| `--lossless` | 只执行 `ansi + dedup` |
| `--charset` | `auto` / `utf-8` / `utf-16le` / `utf-16be` / `gbk` |
| `--json` | 输出结构化 JSON |
| `--stats` | 把 `X-Rtk-*` 统计写到 stderr |
| `--timeout` | 请求超时秒数，默认 60 |

`run` 模式会保留子进程的退出码。

## 发布

`.github/workflows/release.yml` 是手动触发的发布流程（GitHub Actions）：

1. 修改 `Cargo.toml` 的版本号并提交；
2. 在 Actions → **Release** → **Run workflow** 中触发，`tag` 留空会自动使用 `v<版本>`；
3. 流程在 `ubuntu-24.04` 上编译，产出 `rtk-web-<版本>-linux-x86_64.tar.gz` 和 `SHA256SUMS`；
4. 自动创建对应 GitHub Release 并上传资产，可选 pre-release / draft。

发布包内包含 `rtk-web` 二进制、`rtk_client.py`、systemd 单元文件和 README，要求 glibc >= 2.39。

## 测试

```bash
cargo test
```