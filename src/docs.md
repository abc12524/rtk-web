本服务**只做文本压缩，不执行任何命令**：调用方把 bash / PowerShell / 构建工具的输出发过来，服务返回精简后的文本。核心算法来自 rtk 的 `rtk-engine` 与 `src/cmds` 命令专属过滤器。

服务地址：`http://192.168.30.181:8787`，**无鉴权，仅局域网可用**。请求体上限默认 10 MiB，单次压缩超时默认 30 秒。

## 概览 <font style="background:#F6E1AC;color:#664900">输入文本，输出精简内容</font>

支持三种输入方式：

| 方式 | 接口 | 适用场景 |
| --- | --- | --- |
| 原始文本 | `POST /api/compress` | 管道、stdin、任意文本 |
| JSON | `POST /api/compress/json` | 服务间调用 |
| 文件上传 | `POST /api/compress/file` | 上传 `.log`、`.txt`、`.json` 等文本文件 |

两种压缩策略：

| 策略 | 用法 | 说明 |
| --- | --- | --- |
| 通用链 | 默认 | `ansi → progress → dedup → base64-elide → cap` |
| 命令专属过滤器 | `filter=<name>` | 理解命令输出结构，压缩率更高 |

辅助接口：

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/` | 本说明文档 |
| `GET` | `/health` | 健康检查 |
| `GET` | `/api/formats` | 支持的格式列表 |
| `GET` | `/api/filters` | 已注册的命令专属过滤器 |

## 压缩文本 <font style="background:#F6E1AC;color:#664900">最常用</font>

请求体是原始文本，响应体是压缩后的文本，统计信息放在响应头。

```bash
command 2>&1 | curl -X POST --data-binary @- \
  "http://192.168.30.181:8787/api/compress"

# 指定格式与模式
command 2>&1 | curl -X POST --data-binary @- \
  "http://192.168.30.181:8787/api/compress?format=term&mode=chain"

# 无损模式
command 2>&1 | curl -X POST --data-binary @- \
  "http://192.168.30.181:8787/api/compress?lossless=true"
```

### 请求参数

| 参数 | 默认值 | 说明 |
| --- | --- | --- |
| `format` | `term` | `term` / `json` / `web` / `matches` / `lockfile` / `auto` |
| `mode` | `chain` | `chain`（固定链，可预期）/ `auto`（形状感知，激进有损） |
| `lossless` | `false` | `true` 时只执行 `ansi + dedup`，不截断 |
| `charset` | `auto` | `auto` / `utf-8` / `utf-16le` / `utf-16be` / `gbk` |
| `filter` | 无 | 命令专属过滤器名称，优先于 `format` / `mode` / `lossless` |

### 响应头

```text
X-Rtk-Format: term
X-Rtk-Mode: chain
X-Rtk-Charset: auto
X-Rtk-Lossless: false
X-Rtk-Filter: -
X-Rtk-Lossy: true
X-Rtk-Changed: true
X-Rtk-Chain: ansi>progress>dedup>base64-elide>cap
X-Rtk-Bytes-Before: 11424
X-Rtk-Bytes-After: 6803
X-Rtk-Saved-Percent: 40.4
X-Rtk-Tokens-Before: 2856
X-Rtk-Tokens-After: 1701
```

## JSON 接口

请求与响应都是 JSON，适合服务间调用。

```bash
curl -X POST http://192.168.30.181:8787/api/compress/json \
  -H "Content-Type: application/json" \
  -d '{"text":"a\na\na\na\n","format":"term","mode":"chain"}'
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

请求字段：`text`（必填）、`format`、`mode`、`lossless`、`charset`、`filter`。指定 `filter` 时响应会带 `"filter": "ls"`。

## 文件上传 <font style="background:#F6E1AC;color:#664900">multipart</font>

`multipart/form-data` 的字段名必须是 `file`。

```bash
# 返回压缩文本
curl -X POST http://192.168.30.181:8787/api/compress/file \
  -F "file=@build.log"

# 返回结构化 JSON（包含 filename）
curl -X POST "http://192.168.30.181:8787/api/compress/file?response=json" \
  -F "file=@data.json"

# 指定命令专属过滤器
curl -X POST "http://192.168.30.181:8787/api/compress/file?filter=ls" \
  -F "file=@listing.txt"
```

默认按文件名推断格式：

| 文件名 | 格式 |
| --- | --- |
| `*.json` | `json` |
| `*.html` / `*.htm` | `web` |
| `Cargo.lock` / `*.lock` | `lockfile` |
| 其他（`*.log`、`*.txt`） | `term` |

额外参数 `response`：`text`（默认）返回压缩文本；`json` 返回包含 `filename`、`chain`、`stats` 的结构。

> 只接受文本文件。含 NUL 字节的二进制文件返回 `400`，带 UTF-16 BOM 的文件除外。上传大小受 `RTK_WEB_MAX_BODY` 限制。

## 命令专属过滤器 <font style="background:#F6E1AC;color:#664900">推荐用于结构化输出</font>

传 `filter=<name>` 时，服务调用 rtk `src/cmds` 中的专用解析器，而不是通用链。它理解命令输出结构，会重排、缩写、删冗余，压缩率明显更高。

```bash
# ls -l（支持中英文 locale）
ls -l | curl -X POST --data-binary @- \
  "http://192.168.30.181:8787/api/compress?filter=ls"

# cargo test
cargo test 2>&1 | curl -X POST --data-binary @- \
  "http://192.168.30.181:8787/api/compress?filter=cargo-test"

# git status（需要 porcelain 格式）
git status --porcelain -b | curl -X POST --data-binary @- \
  "http://192.168.30.181:8787/api/compress?filter=git-status"

# 查看全部可用名称
curl http://192.168.30.181:8787/api/filters
```

已注册的过滤器：

| 类别 | 名称 |
| --- | --- |
| 文件与目录 | `ls`、`ls-short`、`tree` |
| Git | `git-status`、`git-log`、`git-diff`、`git-branch`、`git-stash-list`、`gh-pr-checks`、`glab-ci-status` |
| Rust | `cargo-test`（别名 `cargo`）、`cargo-build`、`cargo-clippy`、`cargo-install`、`cargo-nextest` |
| Go | `go-test`、`go-build`、`go-vet` |
| Python | `pytest`、`mypy`、`ruff-check`、`ruff-format`、`sqlfluff-lint`、`pip-list`、`pip-outdated` |
| JS / TS | `tsc`、`eslint`、`pylint`、`lint`、`prettier`、`npm`、`pnpm-install`、`next`、`ng`、`prisma-generate`、`prisma-migrate-dev`、`prisma-migrate-status`、`prisma-migrate-deploy`、`prisma-db-push` |
| PHP | `phpunit`、`pest`（别名 `paratest`、`php-test`）、`phpstan`、`pint`、`ecs` |
| Ruby | `rubocop`、`rubocop-text`、`rspec`、`rspec-text` |
| JVM | `mvn-test`、`mvn-compile`、`mvn-package`、`mvn-quiet`、`gradle-test`、`gradle-connected`、`gradle-lint`、`gradle-dependencies`、`sbt-test`、`sbt-compile`、`sbt-run` |
| 其他 | `ctest`、`deno`、`psql`、`black`、`kubectl-pods`、`kubectl-services`、`json`、`json-schema`、`wc`、`log` |

### 输入格式要求

部分过滤器要求对应命令的特定输出格式，格式不匹配时会回退原文：

| 过滤器 | 期望输入 |
| --- | --- |
| `git-status` | `git status --porcelain -b` |
| `git-log` | rtk 注入的 `---END---` 分隔格式 |
| `go-test` | `go test -json` |
| `eslint` | `eslint -f json` |
| `pylint` | `pylint --output-format=json2` |
| `phpstan` / `pint` / `rubocop` / `ruff-check` | 对应工具的 JSON 输出 |
| `pip-list` / `pip-outdated` | `pip --format=json` |
| `kubectl-pods` / `kubectl-services` | `kubectl -o json` |

> 依赖文件、真实 argv、退出码或 stdout/stderr 分离的过滤器（如 `dotnet binlog`、`uv`）没有注册，因为 Web 服务只接收一段文本，无法还原这些上下文。

## 错误码

| 状态码 | 含义 |
| --- | --- |
| `400` | 参数非法、编码无法解码、JSON / multipart 解析失败、未知 `filter` |
| `413` | 请求体超过 `RTK_WEB_MAX_BODY` |
| `405` | 方法不支持 |
| `500` | 压缩任务失败 |
| `504` | 压缩超时 |

错误响应统一为 JSON：

```json
{"error":"unsupported filter `nope`; see GET /api/filters"}
```

## 安全边界 <font style="background:#FCE4E4;color:#C41D1D">仅局域网可达</font>

服务监听 `0.0.0.0:8787`，**无鉴权**，请勿通过端口转发暴露到公网。

> 过滤器调用链可能触发 rtk 的 tee / recall 逻辑。服务已在 systemd 中设置 `RTK_TEE=0` 与 `RTK_RECALL=0`，避免 Web 请求写 SQLite 或落地原文。

> `rtk-engine` 使用 BUSL-1.1 许可：内部自用生产免费；对外提供托管压缩服务需先获得 rtk-ai 授权。

## 客户端

仓库 `scripts/rtk_client.py` 提供只依赖标准库的 Python 客户端：

```bash
python rtk_client.py text                 # 压缩 stdin
python rtk_client.py file build.log       # 上传文件
python rtk_client.py run -- cargo test    # 执行命令并压缩输出
```

# API 列表

[rtk-web · 使用说明](http://192.168.30.181:8787/) · [健康检查](http://192.168.30.181:8787/health) · [过滤器列表](http://192.168.30.181:8787/api/filters) · [博客首页](https://192.168.30.181:7000/)