# trans_proxy

一个用 Rust 实现的 HTTP/HTTPS 透明转发服务。

它同时监听 `80` 和 `443`，配合客户端 `/etc/hosts` 把目标域名指向代理机 IP，就可以在客户端不显式配置浏览器代理的情况下实现访问转发。

## 工作原理

### HTTP

程序监听 `80` 端口，读取请求头中的 `Host`，再把连接转发到真实目标站点的 `80` 端口。

### HTTPS

程序监听 `443` 端口，读取 TLS ClientHello，从中解析 `SNI`，再把 TLS 加密流原样转发到真实目标站点的 `443` 端口。

注意：

- 不解密 HTTPS
- 不做 MITM
- 只依赖 `Host` 和 `SNI` 做目标识别

## 适用场景

- 某些域名无法直接访问，但客户端可以通过修改 `/etc/hosts` 指向一台可出网的中转服务器
- 希望客户端尽量少改动，不额外配置浏览器或系统代理
- 需要一个轻量的透明域名转发服务

## 项目结构

- [src/main.rs](d:/code/rust/trans_proxy/src/main.rs): 启动入口
- [src/config.rs](d:/code/rust/trans_proxy/src/config.rs): 配置加载与环境变量覆盖
- [src/server.rs](d:/code/rust/trans_proxy/src/server.rs): 监听端口与连接处理
- [src/protocol/http.rs](d:/code/rust/trans_proxy/src/protocol/http.rs): HTTP 请求头读取、Host 解析、上游代理请求改写
- [src/protocol/tls.rs](d:/code/rust/trans_proxy/src/protocol/tls.rs): TLS ClientHello 与 SNI 解析
- [src/upstream.rs](d:/code/rust/trans_proxy/src/upstream.rs): 上游 HTTP 代理支持

## 客户端使用方式

在客户端 `/etc/hosts` 中把目标域名指到这台服务的 IP。

例如代理机 IP 为 `192.168.1.10`：

```text
192.168.1.10 example.com
192.168.1.10 www.example.com
192.168.1.10 api.example.com
```

之后客户端直接访问：

```text
http://example.com
https://www.example.com
https://api.example.com
```

请求会先到这台代理机，再由服务按域名转发到真实目标站点。

## 配置方式

支持两种配置来源：

- 配置文件 `trans_proxy.toml`
- 环境变量

环境变量优先级高于配置文件。

### 配置文件示例

可参考 [trans_proxy.toml.example](d:/code/rust/trans_proxy/trans_proxy.toml.example)：

```toml
http_bind = "0.0.0.0:80"
https_bind = "0.0.0.0:443"
client_ip_whitelist = ["192.168.1.*", "10.0.0.5"]

[upstream_http_proxy]
enabled = false
address = "127.0.0.1:3128"
username = ""
password = ""
```

### 环境变量

- `TRANS_PROXY_CONFIG`: 指定配置文件路径
- `HTTP_BIND`: HTTP 监听地址
- `HTTPS_BIND`: HTTPS 监听地址
- `CLIENT_IP_WHITELIST`: 客户端 IP 白名单，多个值可用英文逗号或分号分隔
- `UPSTREAM_HTTP_PROXY_ENABLED`: 是否启用上游 HTTP 代理，支持 `true/false/1/0/yes/no/on/off`
- `UPSTREAM_HTTP_PROXY_ADDR`: 上游 HTTP 代理地址，例如 `127.0.0.1:3128`
- `UPSTREAM_HTTP_PROXY_USERNAME`: 上游 HTTP 代理用户名
- `UPSTREAM_HTTP_PROXY_PASSWORD`: 上游 HTTP 代理密码

## 客户端 IP 白名单

程序支持按客户端 IP 做白名单控制。

### 行为

- 白名单为空时，默认允许所有客户端
- 白名单非空时，只有命中的客户端 IP 才允许继续转发
- 未命中时会直接拒绝连接，不进入 HTTP 或 HTTPS 转发流程

### 通配规则

支持简单模糊匹配：

- `*`: 匹配任意长度字符
- `?`: 匹配单个字符

常见示例：

- `192.168.1.*`: 允许 `192.168.1.` 网段
- `10.0.0.5`: 只允许单个 IP
- `172.16.*`: 允许前缀匹配
- `2001:db8::*`: 允许一段 IPv6 前缀

### 配置文件示例

```toml
http_bind = "0.0.0.0:80"
https_bind = "0.0.0.0:443"
client_ip_whitelist = ["192.168.1.*", "10.0.0.5", "2001:db8::*"]

[upstream_http_proxy]
enabled = false
address = "127.0.0.1:3128"
```

### 环境变量示例

```powershell
$env:CLIENT_IP_WHITELIST="192.168.1.*,10.0.0.5,2001:db8::*"
```

## 上游 HTTP 代理模式

程序支持一个可选的上游 HTTP 代理开关。

### 关闭时

默认直连真实目标站点：

- HTTP 直接连接目标 `host:80`
- HTTPS 直接连接目标 `host:443`

### 开启时

所有对外请求先经过一次上游 HTTP 代理：

- HTTP 请求会改写为绝对 URI 形式，再发送给上游 HTTP 代理
- HTTPS 会先向上游 HTTP 代理发起 `CONNECT host:443`，建隧道后再透传 TLS 流量

### 示例

```toml
http_bind = "0.0.0.0:80"
https_bind = "0.0.0.0:443"
client_ip_whitelist = ["192.168.1.*", "10.0.0.5"]

[upstream_http_proxy]
enabled = true
address = "127.0.0.1:3128"
username = "user"
password = "pass"
```

也可以只用环境变量：

```powershell
$env:UPSTREAM_HTTP_PROXY_ENABLED="true"
$env:UPSTREAM_HTTP_PROXY_ADDR="127.0.0.1:3128"
$env:UPSTREAM_HTTP_PROXY_USERNAME="user"
$env:UPSTREAM_HTTP_PROXY_PASSWORD="pass"
```

## 运行方法

### 开发环境

```powershell
cargo run
```

如果需要使用配置文件：

```powershell
$env:TRANS_PROXY_CONFIG="D:\code\rust\trans_proxy\trans_proxy.toml"
cargo run
```

### Linux 上直接运行

```bash
cargo build --release
sudo ./target/release/trans_proxy
```

或者给程序绑定低位端口能力：

```bash
sudo setcap 'cap_net_bind_service=+ep' ./target/release/trans_proxy
./target/release/trans_proxy
```

## 验证方式

### HTTP

客户端 `/etc/hosts` 指向代理机后：

```bash
curl -v http://example.com/
```

### HTTPS

```bash
curl -v https://example.com/
```

如果服务日志中出现类似输出，说明已经按域名识别并开始转发：

```text
[Http] 192.168.1.20:52314 -> example.com:80
[Https] 192.168.1.20:52315 -> example.com:443
```

## 限制说明

- HTTPS 依赖客户端发送 `SNI`
- HTTP 依赖请求头中存在 `Host`
- 不能处理纯 IP 访问但又希望映射到其他域名的场景
- 不支持 SOCKS 上游代理，目前只支持上游 HTTP 代理
- 不做缓存、重试、负载均衡和访问控制

## 编译检查

当前代码已通过：

```powershell
cargo check
```
