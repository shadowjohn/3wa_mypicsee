# 看快寶 · 3wa_mypicsee

給大圖與連拍照片的離線桌面看圖工具。開檔時先顯示指定圖片，背景才掃同一資料夾。檔名依自然數字排序，例如 `IMG_2.jpg` 在 `IMG_10.jpg` 前。前後的預覽會依瀏覽方向預讀；快取有記憶體上限，不建立磁碟縮圖資料庫，不改寫原圖、不自動連網。

作者：阿寶 ｜ 點子：[羽山](https://3wa.tw/)

## 試用版操作

| 操作 | 功能 |
| --- | --- |
| 拖入圖片、`Ctrl+O`，或用程式開啟檔案 | 以該圖片開始瀏覽所在資料夾 |
| 上下滾輪、方向鍵、Page Up／Page Down | 上一張／下一張 |
| `*` | 將當前圖縮到視窗可裝下的倍率，並鎖住倍率 |
| `+`／`-` | 放大／縮小並鎖住倍率 |
| 左鍵拖曳 | 移動圖片 |
| `F11`／`Esc` | F11 切換無標題列全螢幕，Esc 退出 |
| 右上 About | 看咪卡、版本、作者、MIT 授權及 3wa／GitHub 連結 |
| `P` | 切換翻圖時是否保留拖曳位置 |

JPG／JPEG、PNG、WebP、GIF、TIFF、BMP。試用版 GIF 與多頁 TIFF 只顯示第一幀／頁。放大時可補讀較高解析的預覽，但這不是超大原圖的分塊檢視器；過大的圖或損壞的圖會報錯，仍可繼續翻圖。

## 咪卡的一天

**上午｜書桌前整理照片**

![上午｜書桌前整理照片](snapshot/mika/01-desk.png)

**午後｜廚房裡的一幕**

![午後｜廚房裡的一幕](snapshot/mika/02-kitchen.png)

**晚上｜回到桌前翻看今天的照片**

![晚上｜回到桌前翻看今天的照片](snapshot/mika/03-night.png)

## 本機開發

在 Windows 安裝 Rust 與 MSVC C++ 開發工具後，於專案根目錄執行：

```powershell
cargo test --locked
cargo run --locked --release -- "C:\Photos\IMG_001.jpg"
```

試用版識別為 `20261122.001`，Cargo 版本獨立採 SemVer。圖示與已定稿的 3wa 袖章版「咪卡」放在 `assets/`；生成提示見 [snapshot/avatar/avatar_prompt.md](snapshot/avatar/avatar_prompt.md)。

MIT，見 [LICENSE](LICENSE)。正式發行前依實際打包的套件和內嵌素材彙整第三方授權。自動更新、GIF 動畫、多頁 TIFF 與縮圖牆不在目前試用版。
## Windows 打包與發版

- 本機使用 `build.bat` 編譯 Windows x64，產物在 `dist\3wa_mypicsee.exe`。
- GitHub Actions 會在 main 更新與 PR 上跑 Windows 編譯和測試，不會建立版本。將 `RELEASE_VERSION` 更新成 `YYYYMMDD.NNN`，再推送同值加 `v` 前綴的 tag，才會產生 GitHub Release。
- Release 提供免安裝 ZIP、安裝檔與 SHA-256；安裝檔內可勾選加入 Windows「開啟方式」的圖檔格式。不會自動取代預設看圖程式。
