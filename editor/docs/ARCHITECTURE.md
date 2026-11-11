# 编辑器架构约束

## 技术选择

编辑器使用 `wgpu` 自建 GPU 渲染层。GPUI 只作为研究对象，主要借鉴以下思想：

- UI 状态在单一拥有者上更新；
- 后台任务完成后向主线程提交结果；
- 每一帧根据无效区域和状态变化调度，而不是无条件持续重绘；
- 组件通过上下文访问服务，业务对象不直接持有平台资源；
- 输入、布局、绘制分阶段执行，便于性能测量。

不复制 GPUI 的类型体系、宏、组件实现或调度器代码。

## 依赖方向

```text
app -> workbench/ui/editor-core（待建）
app -> platform -> render
ui  -> render-api（待从 render 中抽出）
editor-core -> text（待建）
agent-acp -> editor-core transaction（待建）
```

禁止事项：

- `editor-core` 依赖 `winit` 或 `wgpu`；
- UI 组件直接读写文件或启动进程；
- Renderer 持有 Buffer、Workspace 或 Agent 会话；
- 扩展和 Agent 绕过事务接口修改工作区；
- 为了短期方便将上游 GPUI 类型暴露为公共 API。

## 下一层接口

后续渲染层不接收业务组件，而接收一帧不可变场景：

```text
FrameScene
  clips[]
  quads[]
  shadows[]
  glyph_runs[]
  images[]
```

UI 层负责状态、布局、命中测试、焦点和无障碍树；渲染层只负责批处理、图集、GPU
资源缓存和提交。该边界使 UI 测试不依赖 GPU，也使渲染 benchmark 可以使用合成场景。

