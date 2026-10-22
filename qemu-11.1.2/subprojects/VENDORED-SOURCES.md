# 本地展开的 QEMU 子项目

本研究目录将原有 `.wrap` 文件声明的 24 个缺失直接依赖作为普通源码纳入
主仓库，不使用 Git submodule，也没有为这些目录创建嵌套 `.git`。
既有的 `libvhost-user` 和 `libvduse` 不属于本次导入。

## 版本与完整性

- 不修改原有 wrap 的版本：Git 项目使用指定 revision 的源码归档；
  Rust crate 使用指定版本归档，并核对 wrap 的 `source_hash`。
- `VENDORED-SOURCES.json` 记录每个项目的来源、revision、下载归档
  SHA-256、文件数和展开后的源码树摘要。
- 对声明了 `patch_directory` 的项目，将本目录既有 `packagefiles/`
  中的 Meson 构建文件复制到展开后的项目中。这与 wrap 的覆盖机制一致，
  因而删除下载缓存后仍可直接配置。
- 保留上游许可证。初次导入时省略的 Git 控制文件逐项记入清单；归档导入
  本身不会产生 `.git`。若以后改用 Git 检出，`.git` 仅保留在本地，不能
  将目录作为 gitlink 提交。

展开后的子项目中有些路径被原有 `subprojects/.gitignore` 忽略。本次按
文件显式纳入主仓库；文件一旦被跟踪，其后续修改正常显示在主仓库状态中。
后续添加源码文件时，也应注意这些既有忽略规则。

## 范围边界

补齐直接 wrap 依赖不表示所有可选配置都已经构建验证：系统开发库、Python
工具及某些可选项目的 Cargo 传递依赖仍可能另有要求。此次不展开 `roms/`
下用于重新构建固件的 Git 子模块，也不改变任何功能开关来掩盖缺失依赖。

配置验证使用主项目 `build/qemu-deps-20261022/`，指定
`--target-list=loongarch64-softmmu --disable-download --disable-docs
--disable-werror --enable-fdt=internal`。`fdt=internal` 用于验证新补齐的
`dtc`，而非绕过它使用系统 libfdt。具体日志和退出状态保存在
`build/dependency-import-20261022/`，不纳入源码提交。
