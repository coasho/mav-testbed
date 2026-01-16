# MAVLink 测试台 (MavTestbed)

一个功能完善的 MAVLink 通信测试工具，类似于 NetAssistant 但专注于 MAVLink 协议。

## 功能特性

### 🔌 连接管理
- 支持 TCP 客户端/服务端
- 支持 UDP 发送/接收/双向
- 支持串口连接
- 可配置 System ID 和 Component ID

### 📤 发送测试
- 加载 MAVLink XML 定义文件
- 消息列表展开和搜索筛选
- 勾选消息后置顶，方便快速查找
- 点击编辑按钮弹出字段编辑对话框
- 支持配置：
  - 字段值（数字、文本、数组）
  - 枚举值下拉选择
  - 自定义 Header（system_id, component_id）
  - 发送频率 (Hz)
- 配置可保存为测试记录，持久化到配置文件

### 📥 接收检测（类似 QGC MAVLink Inspector）
- 实时显示接收到的消息统计
- 消息计数和接收频率
- 显示最后收到的 Header 信息
- 点击消息查看详细字段值
- 自动刷新

### 🪟 多窗口支持
- 支持同时运行多个程序窗口
- 每个窗口自动分配唯一 ID
- 窗口间互不干扰

## 编译和运行

### 依赖
- Rust 1.70+
- 系统依赖（Linux）：
  ```bash
  sudo apt install libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev
  ```

### 编译
```bash
cargo build --release
```

### 运行
```bash
# 第一个窗口
./target/release/mav-testbed

# 可以再打开一个终端运行第二个窗口
./target/release/mav-testbed
```

## 使用说明

### 1. 加载 MAVLink XML
点击顶部的 "📂 加载XML" 按钮，选择 MAVLink 定义文件（如 `common.xml`）。

### 2. 配置连接
点击 "🔌 连接" 按钮，在弹出的对话框中配置：
- 连接类型（TCP/UDP/串口）
- 主机地址和端口
- System ID 和 Component ID

### 3. 发送测试
1. 在左侧消息列表中搜索和勾选要发送的消息
2. 勾选后消息会置顶显示
3. 点击消息右侧的 ✏ 按钮编辑字段值
4. 设置发送频率
5. 点击 "▶ 开始发送" 开始发送

### 4. 接收检测
切换到 "📥 接收检测" 标签页，可以：
- 查看所有接收到的消息类型和统计
- 点击消息查看详细字段值
- 查看 Header 信息

### 5. 保存/加载配置
- 点击 "💾 保存" 可以保存当前配置到文件
- 点击 "📁 加载" 可以加载之前保存的配置

## 配置文件位置

配置文件存储在程序所在目录的 `.mav_testbed` 文件夹中：
- `config.toml` - 应用配置
- `records/` - 保存的测试记录（JSON 格式）

## 项目结构

```
mav_testbed/
├── Cargo.toml          # 项目配置
├── common.xml          # 示例 MAVLink XML 定义
├── README.md           # 说明文档
└── src/
    ├── main.rs         # 程序入口
    ├── ui.rs           # GUI 界面
    ├── config.rs       # 配置管理
    ├── testbed.rs      # 测试台后端逻辑
    └── core/
        ├── mod.rs      # 核心模块入口
        ├── mavlink.rs  # MAVLink 底层传输
        ├── mav_conn.rs # MAVLink 连接管理
        └── mav_mapper.rs # MAVLink XML 解析和消息映射
```

## 依赖的核心模块

本项目复用了三个核心模块：
- `mavlink.rs` - MAVLink 底层传输实现（Serial/TCP/UDP）
- `mav_conn.rs` - MAVLink 连接管理和心跳
- `mav_mapper.rs` - MAVLink XML 解析和消息构建/解析

这些模块来自原项目，提供了完整的 MAVLink 通信能力。

## License

MIT
