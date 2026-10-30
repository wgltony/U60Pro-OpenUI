# U60Pro-OpenUI

[简体中文](README.md) | 繁體中文 | [English](README.en.md)

**中興 U60 Pro（MU5250，CN B31 韌體）** 的第三方介面：一個網頁控制台和一個觸控螢幕選單（DevUI），
在原廠韌體之上加入 Clash / WireGuard / Tailscale 代理、無線中繼、eSIM 管理、Wi-Fi 用戶端管理等功能。
滿足下列前提後，可用主安裝腳本安裝；不等於已完成目前版本的乾淨原廠機驗收。

本倉庫是 [acloudpeng/U60Pro-OpenUI](https://github.com/acloudpeng/U60Pro-OpenUI) 的分支，
延續 [MU5250-OpenUI](https://github.com/dklasens/MU5250-OpenUI) 以及[致謝](#致謝)中列出的工作。

![網頁控制台：首頁、無線中繼、Clash 與 Tailscale 設定、用戶端、DevUI 設定](docs/images/readme/zh/web-tour.gif)

| 服務 | 位址 |
| --- | --- |
| 網頁控制台 | `http://192.168.0.1:8080/` |
| 需要登入的 API | `http://192.168.0.1:9090/` |
| SSH（只接受金鑰） | `ssh -p 2222 root@192.168.0.1` |

> [!WARNING]
> 只在 `MU5250_HW1.0` + `CN_ZTE_MU5250V1.0.0B31` 上測試過。安裝會開啟 ADB、關閉韌體自動更新，
> 並修改裝置的開機腳本。動手之前請先讀[安全說明](docs/SAFETY.md)（英文）。

## 一條指令安裝

需要一台 Mac 或 Linux 電腦。**鎖定 B31 首次安裝必須連接 USB 資料傳輸線，並能透過網路存取路由器**（Wi-Fi 有助於維持網路連線）；只連 Wi-Fi 不夠。已有 root SSH 的更新可以只走 Wi-Fi。

**下載安裝檔案前**，先取得路由器管理員密碼和外部提供的備份金鑰後綴。倉庫不提供後綴；可向[支援管道](https://github.com/wgltony/U60Pro-OpenUI/issues)詢問目前取得指引，不要公開後綴或備份。無法取得就先停止；已知 B31 不能用簡單 `strings` 提取，離線工具也不能代為取得。

```bash
git clone https://github.com/wgltony/U60Pro-OpenUI.git
cd U60Pro-OpenUI
bash install.sh --dry-run     # 先試執行，不寫入裝置；會下載並產生本機材料
bash install.sh               # 安裝；加 --with-tailscale 同時安裝 Tailscale
```

腳本會檢查電腦上的工具，下載本倉庫的發行檔案並校驗，裝置仍鎖定時先開啟 ADB（需要路由器管理員密碼和備份金鑰後綴），
然後透過帶快照及失敗復原嘗試的部署裝好下面所有功能。逐步說明和常見問題見[安裝教學](docs/INSTALL.zh.md)（簡體中文）。

鎖定首次安裝復原前，互動執行必須輸入 `YES` 確認 USB 資料傳輸線已接到本機；非互動執行須明確加 `--acknowledge-usb`。空的 `adb devices` 清單不是接線證明；若有多台裝置，用 `--adb-serial` 指定，等待及部署都沿用該選擇。

之後更新：`bash install.sh --update`（保留密碼，略過沒有變更的部分）。

## 介面一覽

截圖皆來自倉庫內建的示範資料（`web-app/tools/mock_agent.py` 和觸控螢幕的預覽建置），不含真實裝置資訊。
介面語言為簡體中文和英文，截圖使用簡體中文介面。

### 網頁控制台

| | |
| --- | --- |
| ![首頁](docs/images/readme/zh/web-home.png)<br>**首頁**：訊號、速率、電池、載波和快捷操作 | ![無線中繼](docs/images/readme/zh/web-relay.png)<br>**無線中繼**：連接上游 Wi-Fi、儲存多個網路、掃描附近網路 |
| ![Clash 設定精靈](docs/images/readme/zh/web-proxy-clash.png)<br>**Clash**：分步設定精靈、訂閱、節點、依裝置分流 | ![Tailscale](docs/images/readme/zh/web-proxy-tailscale.png)<br>**Tailscale**：網頁上登入、出口節點、遠端管理 |
| ![WireGuard](docs/images/readme/zh/web-proxy-wireguard.png)<br>**WireGuard**：匯入設定、依裝置分流、通道狀態 | ![用戶端](docs/images/readme/zh/web-clients.png)<br>**用戶端**：Wi-Fi / USB / 網路線裝置，一鍵封鎖與解除 |
| ![訊號](docs/images/readme/zh/web-signal.png)<br>**訊號**：5G / LTE 載波、基地台資訊、頻段與基地台鎖定 | ![eSIM](docs/images/readme/zh/web-esim.png)<br>**eSIM**：切換、下載、重新命名、刪除設定檔 |
| ![DevUI 設定](docs/images/readme/zh/web-devui.png)<br>**DevUI**：開啟觸控選單、開啟方式、亮度與閒置設定 | ![工具](docs/images/readme/zh/web-tools.png)<br>**系統 → 工具**：路由器日誌、訊號與連線事件記錄 |

### 觸控螢幕（DevUI）

<p align="center"><img src="docs/images/readme/zh/screen-tour.gif" width="240" alt="觸控選單示範"></p>

| | | | |
| --- | --- | --- | --- |
| ![主選單](docs/images/readme/zh/screen-menu.png)<br>主選單（九宮格） | ![無線中繼](docs/images/readme/zh/screen-relay.png)<br>無線中繼 | ![鍵盤](docs/images/readme/zh/screen-keyboard.png)<br>螢幕鍵盤輸入密碼 | ![已連線裝置](docs/images/readme/zh/screen-wifi-devices.png)<br>已連線裝置與踢出 |
| ![Tailscale](docs/images/readme/zh/screen-tailscale.png)<br>Tailscale 出口節點 | ![Clash](docs/images/readme/zh/screen-clash.png)<br>Clash 模式與節點 | ![WireGuard](docs/images/readme/zh/screen-wireguard.png)<br>WireGuard | ![eSIM](docs/images/readme/zh/screen-esim.png)<br>eSIM 切換 |
| ![訊號](docs/images/readme/zh/screen-signal.png)<br>訊號、網路模式與速率曲線 | ![電池](docs/images/readme/zh/screen-battery.png)<br>電池與充電上限 | ![概覽](docs/images/readme/zh/screen-overview.png)<br>概覽與 USB 網路共享 | ![電源選單](docs/images/readme/zh/screen-power-menu.png)<br>長按電源鍵選單 |

## 功能

- **代理**：WireGuard、Clash、Tailscale 三選一轉送流量，切換前確認，失敗自動還原。每種都有分步設定精靈。
  - **Clash**：固定版本的 Clash.Meta（mihomo）核心和地理資料隨安裝一起部署；最多 5 個訂閱，節點群組與測速，
    規則 / 全域 / 直連，依裝置分流，連線與日誌檢視。見 [Clash 文件](docs/CLASH.md)。
  - **WireGuard**：匯入服務商的 `.conf`，儲存多個設定，所有裝置或指定裝置走通道。見 [WireGuard 文件](docs/WIREGUARD.md)。
  - **Tailscale**（`--with-tailscale`）：在網頁上用登入連結或驗證金鑰登入、登出；Tailscale 模式（經出口節點上網）；
    **遠端管理**（子網路由，在任何地方透過 tailnet 存取路由器和區域網路，可與 Clash / WireGuard 同時使用）。
- **無線中繼**：連上既有的 Wi-Fi（家裡、飯店、辦公室）當作上網通道，省下行動數據；上游斷線或無法上網時自動回到行動網路。
  最多儲存 8 個網路依序連線，支援需要網頁驗證的熱點。
  本分支為規避已觀察到的 AP/STA 衝突，中繼時由守護暫停與上游同頻段的熱點，保留另一頻段；這是目前策略，不是對所有韌體或硬體能力的絕對結論。守護會提交 UCI，並在 `/data` 留持久復原標記；開啟前準備另一頻段或 USB 備用管理通道。
  見 [Wi-Fi 中繼文件](docs/WIFI-UPLINK.md)。
- **Wi-Fi 與用戶端**：2.4G / 5G 開關、發射功率；檢視已連線裝置，在網頁或觸控螢幕上封鎖 / 解除封鎖（依裝置名稱識別隨機 MAC）。
- **訊號與行動網路**：載波和基地台詳情、網路模式、頻段與基地台鎖定、APN、簡訊、流量統計、TTL（預設值或自訂 1–255）。
- **eSIM**：管理 SIM 卡槽中可插拔 eUICC 卡（9eSIM、5ber、eSTK.me 等）上的設定檔，功能對標 Android 上的 EasyEUICC：
  網頁上切換、重新命名、刪除，用啟用碼或 QR 碼圖片下載，處理通知；觸控螢幕上切換。基於 lpac。見 [eSIM 文件](docs/ESIM.md)。
- **電池與系統**：充電上限、溫度與資源監控、停止廠商服務與釋放記憶體、關機與重新開機。
- **USB**：USB 網路共享開關，RNDIS / ECM 模式切換。
- **日誌與診斷**：路由器輪替日誌（中繼、Tailscale、封鎖事件），訊號記錄與連線事件記錄（CSV 下載），AT 主控台。
- **觸控選單（DevUI）**：裝置 320×480 螢幕上的選單，語言跟隨網頁。最多顯示九個功能（五個以上排成九宮格），
  在網頁上選擇要顯示哪些。見[螢幕文件](docs/SCREEN.md)。
- **有校驗的部署**：核對裝置身分和檔案校驗碼，保留快照並在觀測到部署失敗時嘗試復原；不是任何失敗都精確還原的保證。新快照包含目前 UCI / 自有 sidecar 生命週期覆蓋，舊快照只有舊覆蓋範圍，見[回退邊界](docs/DEPLOYMENT.md#rollback-scope-and-limits)。

原始碼、離線 / mock 測試與 CI 不等於目前乾淨原廠 B31 的首次安裝、冷啟動、故障復原驗收；實體 eSIM、無線頻段復原、DevUI 監督和 Windows 首次安裝仍需各自的實機證據。

設定不會上傳專案伺服器，但路由器與電腦都含私密材料：電腦保留加密原始復原備份，獨立 dry-run / 保留工作目錄可能留下解密設定；主安裝程式正常結束時清理暫存目錄（異常終止可能來不及清理）。不要公開這些材料，也不要假設原廠備份含 Tailscale 身分。詳見[隱私說明](docs/DEPLOYMENT.md#privacy-and-recovery-material)。

網頁預設為簡體中文，可切換為英文。見[控制台文件](docs/DASHBOARD.md)。

## DevUI 快速上手

1. 在控制台側欄開啟 **DevUI**，點選 **打开 DevUI 屏幕**。
2. 在裝置上直接進入：原廠介面亮著時，用兩根手指按住螢幕不動 2 秒，手指放在原廠介面沒有按鈕的地方。
   另外兩種方式要在網頁上開啟並儲存：
   - **開機自動開啟 DevUI**：開機後等原廠介面啟動完成再開啟，關機充電時不會開啟；
   - **連按四次開啟 DevUI**：2.5 秒內短按四次電源鍵，每兩次之間不超過 0.7 秒。
3. 短按電源鍵讓螢幕休眠或喚醒；長按約 1.5 秒彈出電源選單，可關機、重新開機或返回原廠介面（關機和重新開機會再確認一次）。
   閒置到設定的時間（預設 2 分鐘）後螢幕熄滅，DevUI 保留；也可以改為返回原廠介面。
4. 亮度、閒置時間（30–600 秒）、閒置後的動作、啟動頁和選單功能都在同一個網頁上設定。儲存後顯示和選單立即生效，
   啟動頁在下次開啟 DevUI 時生效。

雙指按住預設開啟，開機開啟和連按快捷方式預設關閉。原廠鎖定畫面擋不住雙指按住和連按：鎖定時這樣操作，同樣會開啟 DevUI。
螢幕上始終有 **原厂界面** 和 **返回**（左上角）。離開 DevUI 不影響網路。

## 裝置

| **型號** | 中興 U60 Pro（MU5250） |
| --- | --- |
| **硬體** | `MU5250_HW1.0` |
| **韌體** | `CN_ZTE_MU5250V1.0.0B31` |
| **晶片** | 高通 Snapdragon X75（SDX75 / SDXPINN），4 × Cortex-A55 @ 2.2 GHz |

以上是開發所用裝置的資訊，不代表相容其他型號、硬體版本或韌體。

## 更新

- **macOS / Linux**：`bash install.sh --update`。
- **Windows**：雙擊倉庫中的 `deploy-windows.cmd`。它把已經裝好、在 2222 連接埠開著 SSH 的裝置更新到與倉庫版本對應的發行版本
  （agent、網頁、觸控螢幕、lpac 和 Clash 核心），保留原本的密碼。選項見 `Get-Help .\scripts\windows\deploy.ps1 -Detailed`。
  用 Windows 首次安裝未經測試。

請勿使用上游的桌面安裝程式（Open U60 Pro Installer）：它安裝的是上游版本，會覆蓋本分支新增的功能。本分支沒有經過驗收的桌面分發；舊 GUI / `setup.sh` 教學均以目前 `install.sh` 為準。

每個[發行版本](https://github.com/wgltony/U60Pro-OpenUI/releases)都列出檔案及其 SHA-256 校驗碼。

<details>
<summary><b>從原始碼建置與手動部署</b></summary>

`bash install.sh --from-source` 會在本機建置所有元件再安裝。需要：

- Rust，帶 `aarch64-unknown-linux-musl` 目標，以及 `cargo-zigbuild`；
- Zig 0.14.1；主安裝腳本與手動建置均使用 `cargo zigbuild`，不要求另裝 musl 交叉 GCC；
- Python 3；
- Node.js `^20.19.0 || >=22.12.0` 和 npm；
- Clash 核心需要 Go 或 Docker。

也可以手動建置：

```sh
cargo zigbuild --locked --release -p zte-agent --target aarch64-unknown-linux-musl
(cd web-app && npm ci && npm run build)
mkdir -p build && tar czf build/dashboard-dist.tar.gz -C web-app/dist .
ZIG=/path/to/zig sh screen/build.sh
ZIG=/path/to/zig python3 scripts/prepare-lpac.py
python3 scripts/prepare-mihomo.py
python3 scripts/prepare-geodata.py
python3 scripts/prepare-wireguard.py
python3 scripts/prepare-tailscale.py      # 選用
```

螢幕程式、lpac、Clash 核心、WireGuard 工具和 Tailscale 的下載都固定了校驗碼。這些指令都不會連線或修改你的裝置。

用 `deploy-components.py` 部署到已經裝好 OpenUI 的裝置：

```sh
python3 scripts/deploy-components.py \
  --gateway 192.168.0.1 \
  --ssh-key ~/.ssh/id_ed25519 \
  --agent target/aarch64-unknown-linux-musl/release/zte-agent \
  --dashboard build/dashboard-dist.tar.gz \
  --screen build/screen/openui-screen \
  --lpac build/lpac/lpac \
  --mihomo build/mihomo/mihomo --mihomo-geodata build/mihomo/geo \
  --wireguard-tools build/wireguard/wg \
  --keep-credentials --skip-unchanged \
  --dry-run
```

- `--keep-credentials`：保留裝置上現有的 agent 密碼和 PIN。
- `--skip-unchanged`：略過裝置上已有、校驗碼相同的元件。
- 先看試執行的結果，沒問題再拿掉 `--dry-run`。替換螢幕程式之前，先讓螢幕回到原廠介面。
- 新的螢幕程式要先通過一次有人看守的檢查（約三分鐘），網頁上的螢幕入口才會開放：
  `python3 scripts/verify-screen.py --gateway 192.168.0.1 --port 2222 --ssh-key ~/.ssh/id_ed25519`。

其他選項見[部署文件](docs/DEPLOYMENT.md)和[螢幕文件](docs/SCREEN.md)（英文）。

</details>

<details>
<summary><b>本機示範與檢查</b></summary>

不需要裝置。分別在兩個終端機中執行：

```sh
python3 web-app/tools/mock_agent.py --port 19090 --wireguard-demo
```

```sh
cd web-app
npm ci
npm run demo
```

開啟 Vite 給出的網址，用任意非空密碼登入。兩個服務都只監聽本機，模擬資料只存在記憶體中。

```sh
cargo test --locked -p zte-agent -p process-runner
python3 -m unittest discover -s tests -v
python3 scripts/check-api-contract.py
python3 scripts/check-device-secrets.py
(cd web-app && npm run build && npm run lint && npm test)
sh screen/build.sh --preview      # 觸控螢幕的本機預覽與自我測試，渲染圖在 build/screen/
```

</details>

## 文件

- [安裝教學](docs/INSTALL.zh.md)（簡體中文）/ [Install guide](docs/INSTALL.md)
- [開啟 ADB](docs/ADB.zh.md)（簡體中文）/ [Enabling ADB](docs/ADB.md)：原廠 B31 開啟 ADB 的原理和步驟
- 以下為英文：[部署](docs/DEPLOYMENT.md)、[安全](docs/SAFETY.md)、[控制台](docs/DASHBOARD.md)、[觸控螢幕](docs/SCREEN.md)、
  [Clash](docs/CLASH.md)、[WireGuard](docs/WIREGUARD.md)、[Wi-Fi 中繼](docs/WIFI-UPLINK.md)、[eSIM](docs/ESIM.md)、
  [Agent API](docs/AGENT.md)、[操作手冊](docs/OPERATIONS.md)

## 致謝

- [acloudpeng/U60Pro-OpenUI](https://github.com/acloudpeng/U60Pro-OpenUI)：本分支的上游。
- [dklasens/MU5250-OpenUI](https://github.com/dklasens/MU5250-OpenUI)：OpenUI 的後端、控制台、安裝程式和部署基礎。
- [ZweiChen](https://github.com/czwstc)：上游延續的分支，包括中英文觸控選單。
- [jesther-ai/open-u60-pro](https://github.com/jesther-ai/open-u60-pro)：最初的 U60 開源專案，以及 OpenUI 所依賴的裝置存取工作。
- [amenekowo/mu5250_tweaking](https://github.com/amenekowo/mu5250_tweaking)：MU5250 硬體研究，以及觸控面板使用的 Atomic DRM 範例。
- [33333s/u60pro-devui](https://github.com/33333s/u60pro-devui)：DevUI 選單使用的 HTML/CSS RGB565 渲染器。
- [scoltzero/u60pro-devui-remix](https://github.com/scoltzero/u60pro-devui-remix)：觸控選單中裝置控制的參考。
- [estkme-group/lpac](https://github.com/estkme-group/lpac) 和 [OpenEUICC](https://gitea.angry.im/PeterCxy/OpenEUICC)：
  本專案執行的 eSIM LPA，以及 eSIM 頁面所參照的應用程式。
- [SadykovIV/ZTE_U60Pro](https://github.com/SadykovIV/ZTE_U60Pro)：在 B31 上驗證過的 eSIM 切換流程。
- [Clash.Meta / mihomo](https://github.com/chen08209/Clash.Meta)、[Tailscale](https://github.com/tailscale/tailscale)、
  [WireGuard](https://www.wireguard.com/)，以及 litehtml、FreeType 和 GNU Unifont。

原作者和授權聲明皆已保留。本倉庫不包含中興韌體、私人設定或原始參考壓縮檔。

## 版權與授權

各部分的版權歸各自作者：
- OpenUI：David Klasens 和 Jesther Silvestre；
- 上游延續的分支：ZweiChen；
- 上游新增的內容：acloudpeng；
- 本分支新增的內容：wgltony。

| 元件 | 授權 |
| --- | --- |
| OpenUI agent、控制台和通用工具 | [MIT](LICENSE)，保留 David Klasens 和 Jesther Silvestre 的聲明 |
| 單獨建置的螢幕程式和改編的 DRM 程式碼 | [GPL-3.0-or-later](screen/LICENSE) |
| 內嵌的 GNU Unifont OTF 字型 | [SIL Open Font License 1.1](screen/FONT-LICENSE) |
| lpac（單獨的 eSIM 程式） | [AGPL-3.0-only](licenses/lpac/)，其中 libeuicc 為 LGPL-2.1-only，cJSON 為 MIT |
| Clash 核心 mihomo（單獨的程式） | GPL-3.0，原始碼見 [chen08209/Clash.Meta](https://github.com/chen08209/Clash.Meta) 和 `scripts/mihomo-patches/` |
| 相依套件與下載的工具 | 各自上游的授權 |

見 [NOTICE](NOTICE)、[螢幕程式聲明](screen/NOTICE)和[保留的上游授權](licenses/)。根目錄的 MIT 授權不會改變 GPL 螢幕程式碼或第三方字型的授權。
散布 GPL 螢幕程式時，必須附上對應的原始碼和相關聲明；螢幕程式的原始碼和建置腳本都在本倉庫中。
