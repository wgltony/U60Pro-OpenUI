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
- 每次部署保留快照，并在观测到失败时尝试恢复。新快照包含当前 UCI / 自有 sidecar 生命周期覆盖，旧快照只有旧覆盖范围；不保证所有故障或断电都精确恢复。见[回退边界](DEPLOYMENT.md#rollback-scope-and-limits)。
- 个人配置不会上传项目服务器。路由器本地保存密码、Wi-Fi、订阅和身份；**电脑也保留私密的加密原始恢复备份和身份记录**。独立 `zunlock.py --dry-run` / `--keep-work` 可能留下解密配置。主安装器正常成功 / 失败退出时清理临时目录，但另行保留加密恢复备份。两类材料都不要公开，也不要假设原厂备份包含 Tailscale 状态。见[隐私说明](DEPLOYMENT.md#privacy-and-recovery-material)。

**需要准备：**

- 一台 Mac 或 Linux 电脑。用 Windows 第一次安装没有测试过；更新时双击 `deploy-windows.cmd` 即可。
- **锁定 B31 首装必须连接 USB 数据线，并能通过网络访问路由器**。Wi-Fi 有助于保持原厂网页可达，但只连 Wi-Fi 不能提供恢复后的 USB ADB。已有 root SSH 的更新可以只走 Wi-Fi。
- 工具：`python3`、`openssl`、`adb`、`curl`、`ssh`。
  - macOS：`brew install python openssl android-platform-tools`
  - Debian/Ubuntu：`sudo apt-get install python3 openssl adb curl openssh-client`
- 只有锁定的设备才需要（B28 及以后的固件，包括 B31）：
  - 路由器的**管理员密码**（登录 `http://192.168.0.1` 用的密码）；
  - 这一系列设备的**备份密钥后缀**。它不公开发布；**下载安装文件前先取得**，可向本分支[支持渠道](https://github.com/wgltony/U60Pro-OpenUI/issues)询问当前获取指引，不要公开秘密。无法取得就先停止。
    开启 ADB 的原理和步骤（自动、脚本、完全手动三种方式）见 [ADB.zh.md](ADB.zh.md)；
    后缀说明见 [ADB.zh.md](ADB.zh.md#备份密钥后缀)。

已知 B31 不能用简单 `strings` 提取后缀；离线打补丁工具也需要先取得后缀，不能替你获取。无需也不提供作者的私人环境文件。源码、mock 和 CI 不等于当前干净原厂 B31 首装 / 冷启动、实体 eSIM 或 Windows 首装验收。本分支没有经过验收的桌面分发；旧 GUI / setup 教程以这里的 `install.sh` 流程为准，不推荐上游桌面包。

## 1. 获取代码

```sh
git clone https://github.com/wgltony/U60Pro-OpenUI.git
cd U60Pro-OpenUI
```

`install.sh` 会从你克隆的仓库下载对应的发布文件。

## 2. 连接路由器

锁定首装时，先接好 **USB 数据线**，并建立到原厂网页的网络连接（可用 Wi-Fi）；**恢复前确认**线材已接好。ADB 通常要恢复重启后才出现。已有 SSH 的更新可只连 Wi-Fi。以下命令均在仓库根目录执行：

```sh
adb devices
```

- 列出一台状态为 `device` 的设备：ADB 已经可用，跳过解锁。
- 什么都没列出：锁定的路由器就是这样，`install.sh` 会解锁它。

## 3. 先试运行（不写入设备）

```sh
bash install.sh --dry-run
```

它会检查电脑上的工具，下载并校验所有文件；ADB 已经可用时，还会核对路由器的身份和剩余空间。
锁定的路由器会运行解锁工具自己的试运行，在恢复任何东西之前停下；仍会生成本机备份 / 工作材料。通过不代表之后恢复或冷启动必定成功。

## 4. 安装

```sh
bash install.sh                    # 需要 Tailscale 时加 --with-tailscale（约 60 MB）
```

它会询问：

1. **网页控制台的密码**。设一个新密码，用来保护 `http://192.168.0.1:8080` 和 agent API。
2. 只有锁定的路由器才问：**路由器管理员密码**和**备份密钥后缀**。脚本会先显示将要恢复的内容，
   先确认 USB 数据线已连接，征得你同意后才执行；之后路由器重启，仍需线材、驱动和 USB 枚举正常，电脑才能看到 ADB。

恢复前，交互式 `install.sh` 要求输入 `YES`，确认 USB 数据线已接到本机。非交互锁定首装须显式传入 `--acknowledge-usb`；这是你对接线的确认，不是电气检测。空的 `adb devices` 列表不证明已接线。可用 `--adb-serial SERIAL` 保留指定设备，等待 ADB 和部署均沿用选择；没有接好就不要确认。

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
**cargo-zigbuild**、Zig 0.14.1、Python 3、Node.js `^20.19.0 || >=22.12.0` 和 npm；agent 用 `cargo zigbuild`，不要求另装 musl 交叉 GCC；Clash 内核还需要 Go 或 Docker，没有就不装 Clash 内核）。

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
- **无线中继**：把已有的 Wi-Fi 当作上网通道。本分支为规避已观测到的 AP/STA 冲突，
  中继时由守护暂停与上游同频段的热点，保留另一频段；这是当前策略，不是对所有固件或硬件能力的绝对结论。**开启前准备另一频段或 USB 备用管理通道**，不要假设客户端会自动漫游。守护会提交 UCI，并在 `/data/local/tmp/relay-band-guard.disabled` 留下跨重启恢复标记，见[中继边界](WIFI-UPLINK.md#band-guard-and-management-access)。
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
| 部署中途停止 | 它会尝试快照恢复；如果未完成，会打印一条恢复命令（`sh /data/local/tmp/open-u60-transaction.sh restore …`），在路由器上运行它。 |
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
