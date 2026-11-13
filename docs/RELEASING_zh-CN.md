# 发布 Auriscope（Releasing Auriscope）
- 注意：如有差异，以英文为主

所有下游发布产物——包括 GitHub Release 压缩包、AUR 软件包，以及应用内的“发现新版本”通知——都依赖于同一件事：**在 `main` 分支上创建一个带注释的 `vX.Y.Z` 标签（Annotated Tag）。**

请严格按照以下顺序执行。

## 1. 更新版本号（Bump the version）

- `Cargo.toml`：将 `version` 修改为 `X.Y.Z`。然后运行 `cargo build`，使 `Cargo.lock` 同步记录 `auriscope` 软件包自身的新版本号。
- `assets/io.github.frdcmp.Auriscope.metainfo.xml`：在 `<releases>` 标签的最前面添加一个 `<release>` 条目，包含当天的日期和一段简短的 `<description>`。软件中心会展示这段说明，而且 AppStream 检查工具要求最新条目的版本号与即将打标签的版本号一致。
- `packaging/arch/auriscope/PKGBUILD`：将 `pkgver` 设置为 `X.Y.Z`，将 `pkgrel` 设置为 `1`。

检查所有软件包对应的构建方式是否仍然正常：

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo check --no-default-features --features portal   # Portal 文件选择对话框
cargo check --no-default-features --features gtk      # AUR
```

提交本次版本更新：

```bash
git commit -am "Release vX.Y.Z"
```

## 2. 创建标签并推送（Tag and push）

创建带注释的 Git 标签：

```bash
git tag -a vX.Y.Z -m "Auriscope X.Y.Z"
```

将 `main` 分支和标签推送到远程仓库：

```bash
git push origin main vX.Y.Z
```

推送标签会触发 `.github/workflows/release.yml` 工作流。该工作流将自动构建 Linux TAR 压缩包和 Windows ZIP 压缩包，为它们分别生成 `.sha256` 校验文件，并创建一个附带自动生成发布说明的 GitHub Release，将上述四个文件全部作为附件上传。

查看工作流执行情况：

```bash
gh run watch
gh release view vX.Y.Z
```

如果自动生成的发布说明（Release Notes）需要人工补充摘要，可以在 GitHub 上手动编辑。

**必须让 Release 保持已发布（Published）状态，而不能是草稿（Draft）或预发布（Pre-release）。**

原因是应用内的更新检查会读取 `/releases/latest` 接口，而该接口会跳过草稿和预发布版本。

## 3. 发布到 AUR（AUR）

此步骤需要一个 AUR 账户，并且已经配置 SSH 密钥。

进入 `packaging/arch/auriscope/` 目录：

```bash
cd packaging/arch/auriscope/
```

执行以下命令：

### 3.1 更新软件包校验和

```bash
updpkgsums
```

该命令会根据 Git 标签对应的源码压缩包自动填写 SHA256 校验和。

### 3.2 生成 `.SRCINFO`

```bash
makepkg --printsrcinfo > .SRCINFO
```

### 3.3 实际构建软件包

```bash
makepkg -sf
```

需要真正执行一次软件包构建，以验证构建过程正常。

完成后，在当前仓库中提交 `PKGBUILD` 和 `.SRCINFO` 这两个文件。

然后将相同的两个文件推送到以下 AUR 仓库：

```text
ssh://aur@aur.archlinux.org/auriscope.git
```

注意：AUR 仓库的根目录中只保存这两个文件。

`auriscope-git` 软件包不需要随着每次正式版本发布而进行修改。

## 4. Windows 发布（Windows）

GitHub Release 中的 ZIP 压缩包就是 Windows 版本的分发包。

下载了该 ZIP 压缩包的用户，在下一次启动应用时，会收到应用内的新版本通知。

更新通知机制如下：

- 最多每天检查一次更新。
- 检测到新版本后，向用户显示通知。
- 通知会指向对应的 GitHub Release 页面。

作者计划的下一步工作是添加 winget 清单（Manifest），使用户也能够通过 `winget upgrade` 命令完成版本升级。

具体规划请参阅 README 中的路线图（Roadmap）。

## 5. 发布检查清单（Checklist）

正式发布前，请逐项确认：

```text
[ ] Cargo.toml 版本号已更新，Cargo.lock 已重新生成
[ ] metainfo 中已添加 <release> 条目
[ ] PKGBUILD 中的 pkgver / pkgrel 已更新
[ ] fmt、clippy、test 和两种 --no-default-features 检查全部通过
[ ] 已提交 Commit，创建 Annotated Tag，并推送 main 和 Tag
[ ] release.yml 工作流成功，Release 包含四个附件，且不是 Draft
[ ] AUR：已执行 updpkgsums、生成 .SRCINFO 并完成推送
```

## 6. 约定与注意事项（Conventions）

### 6.1 标签命名规则

所有标签均采用 `vX.Y.Z` 格式，并遵循语义化版本规范（Semantic Versioning，SemVer）。

应用内的更新检查会将标签中的数字版本号与 `CARGO_PKG_VERSION` 进行比较。

因此，类似 `v0.2.0-rc1` 这样的标签会被视为：

- 比 `v0.2.0` 更旧。
- 比 `v0.1.9` 更新。

如果发布预发布版本，应将其标记为 GitHub 预发布版本（Pre-release），这样 `/releases/latest` 就会完全跳过该版本。

### 6.2 不要重新指向已经发布的标签

**绝对不要重新指向（Re-point）一个已经发布的标签。**

如果已经发布的版本存在问题，应通过发布一个新的补丁版本来修复，而不是修改或重新指向原来的标签。

例如：

```text
v0.2.0 → v0.2.1
```

也就是说，始终向前修复（Fix forward），不要重写已经发布的版本标签。