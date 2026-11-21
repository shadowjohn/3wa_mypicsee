# 看快寶 · 3wa_mypicsee

給大圖與連拍照片的離線桌面看圖工具。開檔時先顯示指定圖片，背景才掃同一資料夾。檔名依自然數字排序，例如 `IMG_2.jpg` 在 `IMG_10.jpg` 前。前後的預覽會依瀏覽方向預讀；快取有記憶體上限，不建立磁碟縮圖資料庫，不改寫原圖、不自動連網。

## 試用版操作

| 操作 | 功能 |
| --- | --- |
| 拖入圖片、`Ctrl+O`，或用程式開啟檔案 | 以該圖片開始瀏覽所在資料夾 |
| 上下滾輪、方向鍵 | 上一張／下一張 |
| `*` | 將當前圖縮到視窗可裝下的倍率，並鎖住倍率 |
| `+`／`-` | 放大／縮小並鎖住倍率 |
| 左鍵拖曳 | 移動圖片 |
| `P` | 切換翻圖時是否保留拖曳位置 |

JPG／JPEG、PNG、WebP、GIF、TIFF、BMP。試用版 GIF 與多頁 TIFF 只顯示第一幀／頁。放大時可補讀較高解析的預覽，但這不是超大原圖的分塊檢視器；過大的圖或損壞的圖會報錯，仍可繼續翻圖。

## 本機開發

在 Windows 安裝 Rust 與 MSVC C++ 開發工具後，於專案根目錄執行：

```powershell
cargo test --locked
cargo run --locked --release -- "C:\Photos\IMG_001.jpg"
```

試用版識別為 `20261122.001`，Cargo 版本獨立採 SemVer。圖示與已定稿的 3wa 袖章版「咪卡」放在 `assets/`；生成提示見 [snapshot/avatar/avatar_prompt.md](snapshot/avatar/avatar_prompt.md)。

MIT，見 [LICENSE](LICENSE)。正式發行前依實際打包的套件和內嵌素材彙整第三方授權。GitHub Actions 發行、更新、GIF 動畫、多頁 TIFF 與縮圖牆不在目前試用版。
