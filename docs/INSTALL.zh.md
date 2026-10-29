# 安装教程：一条命令从原厂 U60 Pro 装好 OpenUI

简体中文 | [English](INSTALL.md)

本教程把一台原厂固件的中兴 U60 Pro 装成完整的 OpenUI：网页控制台、触摸屏界面（DevUI）、
Clash / WireGuard / Tailscale、无线中继、eSIM、USB 网络共享和路由器日志。实际工作由 `install.sh`
完成，这里说明每一步在做什么，以及装好之后怎么用。

## 开始之前

**只在这台设备上测试过：**

| 型号 | 中兴 U60 Pro（MU5250） |
| --- | --- |
| 硬件 | `MU5250_HW1.0` |
| 固件 | `CN_ZTE_MU5250V1.0.0B31` |

其他固件或硬件版本没有测试过。请先读一遍 [SAFETY.md](SAFETY.md)（英文）。要点：

- 锁定的设备开启 ADB 的方法是：恢复一份打过补丁的、你自己的设置备份，然后路由器重启
  （约 90 秒没有网络）。你的设置会保留。
- 安装会关闭固件自动更新（更新可能让解锁失效），在 2222 端口开启只认密钥的 SSH，
  下次重启后 USB 回到普通的网络共享模式。
- 每次部署都会保留快照，任何一步失败都会自动恢复原样。
- 仓库里没有任何个人信息。你的密码、Wi-Fi、订阅和 Tailscale 身份只保存在你的路由器上。

**需要准备：**

- 一台 Mac 或 Linux 电脑。用 Windows 第一次安装没有测试过；更新时双击 `deploy-windows.cmd` 即可。
- 一根 USB-C 数据线，或者让电脑连上路由器的 Wi-Fi。
- 工具：`python3`、`openssl`、`adb`、`curl`、`ssh`。
  - macOS：`brew install python openssl android-platform-tools`
  - Debian/Ubuntu：`sudo apt-get install python3 openssl adb curl openssh-client`
- 只有锁定的设备才需要（B28 及以后的固件，包括 B31）：
  - 路由器的**管理员密码**（登录 `http://192.168.0.1` 用的密码）；
  - 这一系列设备的**备份密钥后缀**。应发现者的要求，它不公开发布，请向社区询问。
    开启 ADB 的原理和步骤（自动、脚本、完全手动三种方式）见 [ADB.zh.md](ADB.zh.md)；
    后缀说明见 [ADB.zh.md](ADB.zh.md#备份密钥后缀)。

## 1. 获取代码

```sh
git clone https://github.com/wgltony/U60Pro-OpenUI.git
cd U60Pro-OpenUI
```

`install.sh` 会从你克隆的仓库下载对应的发布文件。

## 2. 连接路由器

用 USB-C 线连到电脑，或者连上它的 Wi-Fi。检查：

```sh
adb devices
```

- 列出一台状态为 `device` 的设备：ADB 已经可用，跳过解锁。
- 什么都没列出：锁定的路由器就是这样，`install.sh` 会解锁它。

## 3. 先试运行（不写入任何东西）

```sh
bash install.sh --dry-run
```

它会检查电脑上的工具，下载并校验所有文件；ADB 已经可用时，还会核对路由器的身份和剩余空间。
锁定的路由器会运行解锁工具自己的试运行，在恢复任何东西之前停下。

## 4. 安装

```sh
bash install.sh                    # 需要 Tailscale 时加 --with-tailscale（约 60 MB）
```

它会询问：

1. **网页控制台的密码**。设一个新密码，用来保护 `http://192.168.0.1:8080` 和 agent API。
2. 只有锁定的路由器才问：**路由器管理员密码**和**备份密钥后缀**。脚本会先显示将要恢复的内容，
   征得你同意后才执行；之后路由器重启，回来时 ADB 已开启。

然后在一次部署里装好：

| 安装内容 | 位置 |
| --- | --- |
| Agent（API、代理、中继、USB、Wi-Fi、日志） | `/data/zte-agent`，端口 9090 |
| 网页控制台及其服务 | `http://192.168.0.1:8080` |
| 触摸屏界面 | `/data/bin/openui-screen` |
| Clash 内核（固定版本）和地理数据 | `/data/bin/mihomo`、`/data/share/mihomo/` |
| eSIM 工具（lpac） | `/data/bin/lpac` |
| WireGuard 工具 | `/data/bin/wg` |
| 中继频段守护 | `/data/bin/relay-band-guard.sh` |
| Tailscale（加 `--with-tailscale` 时） | `/data/bin/tailscale*`，服务 `openui-tailscale` |
| SSH（只认密钥） | 端口 2222，密钥 `~/.ssh/id_ed25519`（没有就自动生成） |

路由器自带的 80 端口网页不受影响。重启后所有服务都会自动启动。

想全部自己编译？加 `--from-source`（需要带 `aarch64-unknown-linux-musl` 目标的 Rust、
Zig 0.14.1 和 Node.js 22；Clash 内核还需要 Go 或 Docker，没有就不装 Clash 内核）。

## 5. 触摸屏检查

脚本最后会提出做一次有人值守的触摸屏检查（约三分钟）：它会临时接管屏幕，测试闲置、
崩溃和卡住时能否恢复。请让路由器在你眼前。检查通过之前，控制台上的 **打开 DevUI 屏幕**
按钮保持灰色。以后再做：

```sh
python3 scripts/verify-screen.py --gateway 192.168.0.1 --ssh-key ~/.ssh/id_ed25519 --if-needed
```

## 6. 重启

方便时重启一次路由器。USB 回到普通网络共享，ADB 关闭；之后用 SSH 管理
（`ssh -p 2222 root@192.168.0.1`）。

## 装好之后

打开 `http://192.168.0.1:8080`，用你设的密码登录。

- **代理**：在 WireGuard、Clash、Tailscale 中选一个，同一时间只有一个转发流量。
  每个标签页顶部都有设置向导：
  - *Clash*：内核和地理数据已经装好（以后可在 **高级 → 内核** 更新）。添加服务商的 Clash / mihomo 订阅链接，选设备，开启。
  - *WireGuard*：导入 VPN 服务商给的 `.conf` 文件，选设备，开启。
  - *Tailscale*（安装时加了 `--with-tailscale`）：用页面上的登录链接（在手机或电脑上打开）
    或认证密钥登录。然后二选一：开启 **远程管理**，让你的 tailnet 访问本路由器及其局域网，
    可与 Clash 或 WireGuard 同时使用（在 Tailscale 管理后台批准一次 `192.168.0.0/24` 路由）；
    或者使用 **Tailscale 模式**，通过出口节点上网。
- **无线中继**：把已有的 Wi-Fi 当作上网通道。无线芯片只能在不同频段上同时连接上游和开热点：
  中继连上后，与上游同频段的热点会暂停，另一个频段照常工作（两个频段同名，大多数设备会自动切过去）。
- **网络 → 客户端**：已连接的设备；拉黑或解除拉黑 Wi-Fi 设备（触摸屏上也可以）。
- **概览**：USB 网络共享开关（通过 USB 共享网络）。
- **系统 → 工具 → 路由器日志**：中继、Tailscale 和拉黑事件，保存在路由器上（约 1 MiB）。
- **DevUI**：打开触摸屏界面，设置菜单、启动页和电源键快捷方式。见 README 的 DevUI 一节。

**Wi-Fi 地区（可选）**：CN 固件使用中国的信道规则。在中国以外使用时，通过 SSH 设置你的国家代码
后重启（把 `US` 换成你的国家）：

```sh
ssh -p 2222 root@192.168.0.1 'uci set wireless.wifi0.country=US; uci set wireless.wifi1.country=US; uci commit wireless'
```

## 更新

```sh
git pull
bash install.sh --update            # 保留密码和 PIN，跳过没有变化的部分
```

`--update` 需要 2222 端口的 SSH 或 ADB，不会解锁，也不会重新加固。

## 常见问题

| 问题 | 处理方法 |
| --- | --- |
| `The touchscreen is running OpenUI` | 在触摸屏上点 **原厂界面**，再运行一次；或者加 `--skip-screen`。 |
| `No release found` | 仓库还没有发布版本：加 `--from-source`，或者用 `--repo OWNER/NAME` 指定仓库。 |
| 解锁三分钟后还是没有 ADB | 重新插拔 USB-C 线（要用数据线），运行 `adb devices`；然后再运行一次 `bash install.sh`，ADB 可用时它会接着往下走。 |
| 解锁失败 | 检查管理员密码和后缀，再运行 `bash install.sh --dry-run`。检查通过之前不会恢复任何东西。 |
| 重启后打不开控制台 | `ssh -p 2222 root@192.168.0.1 sh /data/local/tmp/start_dashboard.sh` |
| 部署中途停止 | 它会自动恢复原样；如果没能恢复，会打印一条恢复命令（`sh /data/local/tmp/open-u60-transaction.sh restore …`），在路由器上运行它。 |
| 之后哪里不对劲 | 看 **系统 → 工具 → 路由器日志**，或者通过 SSH 看 `/data/local/tmp/openui-logs/`。 |

更多细节（英文）：[DEPLOYMENT.md](DEPLOYMENT.md)（解锁和部署原理）、[SCREEN.md](SCREEN.md)、
[WIFI-UPLINK.md](WIFI-UPLINK.md)、[CLASH.md](CLASH.md)、[WIREGUARD.md](WIREGUARD.md)。

## 给贡献者：不要提交个人信息

`scripts/check-device-secrets.py`（CI 里也会运行）会拒绝设备身份、私钥、令牌、WireGuard 和
Tailscale 密钥。把你自己的名字、SSID、账号和主机写进 `.git/info/personal-patterns`
（每行一个；这个文件在 `.git` 里面，永远不会被提交），并设成提交前检查：

```sh
printf '#!/bin/sh\nexec python3 scripts/check-device-secrets.py\n' > .git/hooks/pre-commit
chmod +x .git/hooks/pre-commit
```
