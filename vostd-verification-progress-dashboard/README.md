# VOSTD Verification Progress Dashboard

一个不依赖前端框架的静态仪表盘，用来展示 VOSTD 的 Verus 验证进度。

页面直接读取主仓库生成的 `target/verification-progress/progress.json`，并把数据嵌入单个 `index.html`。生成后的页面可以直接用浏览器打开，不需要启动服务器。

## 生成页面

```sh
make
```

如果进度报告不存在，`make` 会先在主仓库执行 `make progress`。

## 刷新数据

```sh
make refresh
```

也可以在主仓库根目录执行：

```sh
make progress-dashboard
```

这会重新运行完整进度统计，再生成本页面。

## 页面内容

- 项目级主覆盖率、契约覆盖率，以及 checked / trusted / unverified 构成。
- x86、RISC-V、LoongArch 分架构统计；未完成整 crate 验证的架构明确标记为“未确认”。
- Proof、Spec、代码行组成、unsafe exec 分布和信任债务。
- 包、子系统、架构的可搜索、筛选、排序、分页明细表。
- 可分享的 URL hash 筛选状态，以及可选的基线变化信息。

## 数据口径

- `checked` 只表示函数体已由本次完整 Verus 构建确认。
- `trusted` 表示函数体位于信任边界，没有被 Verus 检查。
- `unverified` 表示普通 Rust 或尚未由对应架构完整构建确认的函数。
- Proof、Spec 和代码行只反映证明规模，不计入主覆盖率。
