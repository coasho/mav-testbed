// ============================================================================
// MAVLink 测试台 GUI
// ============================================================================

use crate::config::{
    ConfigManager, ConnectionConfig, ConnectionType, FieldValue, SendMessageConfig, SendTestRecord,
};
use crate::testbed::{BackendEvent, MessageStats, UiCommand};
use crossbeam_channel::{bounded, Receiver, Sender};
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use crate::mav_mapper::MavMapper;

/// 日志条目
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub timestamp: String,
    pub message: String,
    pub is_error: bool,
}

/// 接收到的消息记录
#[derive(Debug, Clone)]
pub struct ReceivedMessage {
    pub timestamp: String,
    pub header: mavlink::MavHeader,
    pub msg_id: u32,
    pub msg_name: String,
    pub fields: HashMap<String, f64>,
}

/// 当前标签页
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ActiveTab {
    #[default]
    Send,
    Receive,
    Log,
}

/// 消息编辑对话框
#[derive(Default)]
pub struct MessageEditDialog {
    pub open: bool,
    pub config: SendMessageConfig,
    pub field_values: HashMap<String, String>,
    pub editing_index: Option<usize>,
}

/// 主应用
pub struct MavTestbedApp {
    // 窗口标识
    window_id: u8,

    // 通信
    cmd_tx: Sender<UiCommand>,
    event_rx: Receiver<BackendEvent>,

    // 配置
    config_manager: ConfigManager,
    current_record: SendTestRecord,
    saved_records: Vec<String>,
    record_name_input: String,

    // MAVLink映射器
    mapper: Option<Arc<MavMapper>>,
    xml_path: String,

    // 连接状态
    is_connected: bool,
    is_connecting: bool,  // 新增：连接中状态
    is_sending: bool,
    connection_config: ConnectionConfig,
    connection_id: u64,  // 当前连接ID，用于过滤旧连接的事件

    // 串口列表
    available_ports: Vec<String>,

    // 消息列表
    all_messages: Vec<(u32, String)>,
    search_filter: String,
    selected_messages: HashSet<u32>,
    send_configs: Vec<SendMessageConfig>,

    // 消息编辑对话框
    edit_dialog: MessageEditDialog,

    // 接收统计
    recv_stats: Vec<MessageStats>,
    recv_messages: Vec<ReceivedMessage>,
    selected_recv_msg: Option<u32>,

    // 日志
    logs: Vec<LogEntry>,

    // UI状态
    active_tab: ActiveTab,
    show_connection_dialog: bool,
    show_save_dialog: bool,
    show_load_dialog: bool,

    // 发送统计
    send_stats: HashMap<String, u64>,

    // 后台线程
    _backend_thread: Option<thread::JoinHandle<()>>,
}

impl MavTestbedApp {
    pub fn new(cc: &eframe::CreationContext<'_>, window_id: u8) -> Self {
        // 设置字体
        let mut fonts = egui::FontDefinitions::default();
        #[cfg(target_os = "windows")]
        if let Ok(font_data) = std::fs::read("C:\\Windows\\Fonts\\msyh.ttc") {
            fonts.font_data.insert(
                "microsoft_yahei".to_owned(),
                egui::FontData::from_owned(font_data),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "microsoft_yahei".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("microsoft_yahei".to_owned());
        }
        cc.egui_ctx.set_fonts(fonts);

        // 创建通信通道
        let (cmd_tx, cmd_rx) = bounded::<UiCommand>(32);
        let (event_tx, event_rx) = bounded::<BackendEvent>(256);

        // 启动后台线程
        let backend_thread = thread::spawn(move || {
            let mut backend = crate::testbed::TestbedBackend::new(event_tx, cmd_rx);
            backend.run();
        });

        let config_manager = ConfigManager::new();
        let app_config = config_manager.load_app_config();
        let saved_records = config_manager.list_records();

        let mut app = Self {
            window_id,
            cmd_tx,
            event_rx,
            config_manager,
            current_record: SendTestRecord::default(),
            saved_records,
            record_name_input: String::new(),
            mapper: None,
            xml_path: app_config.xml_path.clone(),
            is_connected: false,
            is_connecting: false,
            is_sending: false,
            connection_config: ConnectionConfig::default(),
            connection_id: 0,
            available_ports: Self::enumerate_serial_ports(),
            all_messages: Vec::new(),
            search_filter: String::new(),
            selected_messages: HashSet::new(),
            send_configs: Vec::new(),
            edit_dialog: MessageEditDialog::default(),
            recv_stats: Vec::new(),
            recv_messages: Vec::new(),
            selected_recv_msg: None,
            logs: Vec::new(),
            active_tab: ActiveTab::Send,
            show_connection_dialog: false,
            show_save_dialog: false,
            show_load_dialog: false,
            send_stats: HashMap::new(),
            _backend_thread: Some(backend_thread),
        };

        // 自动加载XML
        if !app_config.xml_path.is_empty() {
            app.load_xml(&app_config.xml_path);
        }

        app
    }

    /// 枚举可用串口
    fn enumerate_serial_ports() -> Vec<String> {
        match serialport::available_ports() {
            Ok(ports) => {
                let mut names: Vec<String> = ports.into_iter().map(|p| p.port_name).collect();
                names.sort();
                names
            }
            Err(_) => Vec::new(),
        }
    }

    /// 刷新串口列表
    fn refresh_serial_ports(&mut self) {
        self.available_ports = Self::enumerate_serial_ports();
    }

    fn load_xml(&mut self, path: &str) {
        self.xml_path = path.to_string();
        match MavMapper::new(path) {
            Ok(mapper) => {
                self.all_messages.clear();
                for msg_id in mapper.get_all_message_ids() {
                    if let Some(name) = mapper.get_message_name(msg_id) {
                        self.all_messages.push((msg_id, name.to_string()));
                    }
                }
                self.all_messages.sort_by(|a, b| a.1.cmp(&b.1));

                self.mapper = Some(Arc::new(mapper));
                self.log(format!("加载 {} 个消息定义", self.all_messages.len()));

                let _ = self.cmd_tx.send(UiCommand::LoadXml(path.to_string()));
            }
            Err(e) => {
                self.log_error(format!("加载XML失败: {}", e));
            }
        }
    }

    fn process_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                BackendEvent::ConnectionStateChanged(connected, event_conn_id) => {
                    // 只处理当前连接ID的事件，忽略旧连接的事件
                    if event_conn_id != self.connection_id {
                        continue;  // 忽略来自旧连接的事件
                    }

                    if connected {
                        // 只有在"连接中"状态时才接受连接成功事件
                        if self.is_connecting {
                            self.is_connected = true;
                            self.is_connecting = false;
                            self.log("已连接".to_string());
                        }
                    } else {
                        // 断开事件
                        self.is_connected = false;
                        self.is_connecting = false;
                        self.is_sending = false;
                        self.log("已断开".to_string());
                    }
                }
                BackendEvent::MessageReceived(header, msg_id, msg_name, fields) => {
                    // 只有已连接或连接中状态才处理消息
                    if !self.is_connected && !self.is_connecting {
                        continue;  // 忽略断开状态下收到的消息
                    }

                    // 首次收到消息时，如果正在连接中，则确认连接成功
                    if self.is_connecting && !self.is_connected {
                        self.is_connected = true;
                        self.is_connecting = false;
                        self.log("已连接".to_string());
                    }

                    let msg = ReceivedMessage {
                        timestamp: chrono::Local::now().format("%H:%M:%S%.3f").to_string(),
                        header,
                        msg_id,
                        msg_name,
                        fields,
                    };
                    self.recv_messages.push(msg);
                    if self.recv_messages.len() > 1000 {
                        self.recv_messages.remove(0);
                    }
                }
                BackendEvent::StatsUpdated(stats) => {
                    // 只有已连接状态才处理统计更新
                    if !self.is_connected {
                        continue;
                    }
                    // 合并更新而非完全替换，避免UI闪烁
                    for new_stat in stats {
                        if let Some(existing) = self.recv_stats.iter_mut().find(|s| s.msg_id == new_stat.msg_id) {
                            // 更新现有条目
                            existing.count = new_stat.count;
                            existing.rate_hz = new_stat.rate_hz;
                            existing.last_seen = new_stat.last_seen;
                            existing.last_header = new_stat.last_header;
                            existing.last_fields = new_stat.last_fields;
                        } else {
                            // 添加新条目
                            self.recv_stats.push(new_stat);
                        }
                    }
                }
                BackendEvent::Log(msg) => {
                    self.log(msg);
                }
                BackendEvent::Error(msg) => {
                    self.log_error(msg);
                }
                BackendEvent::SendStats { msg_name, count } => {
                    self.send_stats.insert(msg_name, count);
                }
            }
        }

        // 验证选中的消息是否仍然存在
        if let Some(selected_id) = self.selected_recv_msg {
            if !self.recv_stats.iter().any(|s| s.msg_id == selected_id) {
                self.selected_recv_msg = None;
            }
        }
    }

    fn log(&mut self, message: String) {
        self.logs.push(LogEntry {
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            message,
            is_error: false,
        });
        if self.logs.len() > 500 {
            self.logs.remove(0);
        }
    }

    fn log_error(&mut self, message: String) {
        self.logs.push(LogEntry {
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            message,
            is_error: true,
        });
        if self.logs.len() > 500 {
            self.logs.remove(0);
        }
    }

    fn connect(&mut self) {
        self.connection_id += 1;  // 递增连接ID
        self.is_connecting = true;
        // 清空可能的旧事件
        while self.event_rx.try_recv().is_ok() {}
        let _ = self.cmd_tx.send(UiCommand::Connect(self.connection_config.clone(), self.connection_id));
    }

    fn disconnect(&mut self) {
        self.is_connecting = false;
        self.is_connected = false;
        self.is_sending = false;
        // 清空接收数据，防止旧连接的数据残留
        self.recv_stats.clear();
        self.recv_messages.clear();
        self.selected_recv_msg = None;
        // 清空事件队列，防止旧事件覆盖断开状态
        while self.event_rx.try_recv().is_ok() {}
        let _ = self.cmd_tx.send(UiCommand::Disconnect);
    }

    fn start_sending(&mut self) {
        let configs: Vec<_> = self.send_configs.iter().filter(|c| c.enabled).cloned().collect();
        if configs.is_empty() {
            self.log_error("没有启用的发送消息".to_string());
            return;
        }
        let _ = self.cmd_tx.send(UiCommand::StartSending(configs));
        self.is_sending = true;
    }

    fn stop_sending(&mut self) {
        let _ = self.cmd_tx.send(UiCommand::StopSending);
        self.is_sending = false;
    }
}

impl eframe::App for MavTestbedApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_events();
        // 降低刷新频率，减少闪烁
        ctx.request_repaint_after(Duration::from_millis(100));

        // 顶部状态栏
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            self.show_top_bar(ui);
        });

        // 底部状态栏
        egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            self.show_status_bar(ui);
        });

        // 主内容区
        egui::CentralPanel::default().show(ctx, |ui| {
            // 标签页选择
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.active_tab, ActiveTab::Send, "📤 发送测试");
                ui.selectable_value(&mut self.active_tab, ActiveTab::Receive, "📥 接收检测");
                ui.selectable_value(&mut self.active_tab, ActiveTab::Log, "📋 日志");
            });
            ui.separator();

            match self.active_tab {
                ActiveTab::Send => self.show_send_panel(ctx, ui),
                ActiveTab::Receive => self.show_receive_panel(ctx, ui),
                ActiveTab::Log => self.show_log_panel(ui),
            }
        });

        // 对话框
        self.show_dialogs(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.cmd_tx.send(UiCommand::Shutdown);
    }
}

// ============================================================================
// UI 组件实现
// ============================================================================

impl MavTestbedApp {
    /// 顶部工具栏
    fn show_top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("🛩 MAVLink测试台 #{}", self.window_id))
                    .strong()
                    .size(16.0),
            );

            ui.separator();

            // XML加载
            if ui.button("📂 加载XML").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("XML", &["xml"])
                    .pick_file()
                {
                    self.load_xml(&path.to_string_lossy());
                }
            }

            ui.label(format!("消息: {}", self.all_messages.len()));

            ui.separator();

            // 连接控制 - 显示连接地址
            if self.is_connected {
                let addr = self.connection_config.to_addr_string();
                ui.label(egui::RichText::new("● 已连接").color(egui::Color32::GREEN));
                ui.label(egui::RichText::new(format!("[{}]", addr)).monospace().small());
                if ui.button("⛔ 断开").clicked() {
                    self.disconnect();
                }
            } else if self.is_connecting {
                let addr = self.connection_config.to_addr_string();
                ui.label(egui::RichText::new("◐ 连接中...").color(egui::Color32::YELLOW));
                ui.label(egui::RichText::new(format!("[{}]", addr)).monospace().small());
                if ui.button("⛔ 取消").clicked() {
                    self.disconnect();
                }
            } else {
                ui.label(egui::RichText::new("○ 未连接").color(egui::Color32::GRAY));
                if ui.button("🔌 连接").clicked() {
                    self.show_connection_dialog = true;
                }
            }

            ui.separator();

            // 发送控制
            if self.is_connected {
                if !self.is_sending {
                    if ui.button("▶ 开始发送").clicked() {
                        self.start_sending();
                    }
                } else {
                    if ui.button("⏹ 停止发送").clicked() {
                        self.stop_sending();
                    }
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("💾 保存").clicked() {
                    self.show_save_dialog = true;
                }
                if ui.button("📁 加载").clicked() {
                    self.saved_records = self.config_manager.list_records();
                    self.show_load_dialog = true;
                }
            });
        });
    }

    /// 底部状态栏
    fn show_status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // 接收统计 - 使用固定宽度避免布局跳动
            let recv_count = self.recv_stats.len();
            ui.label(format!("接收: {} 种消息", recv_count));
            ui.separator();

            let total_recv: u64 = self.recv_stats.iter().map(|s| s.count).sum();
            ui.label(format!("总计: {} 条", total_recv));
            ui.separator();

            let total_send: u64 = self.send_stats.values().sum();
            ui.label(format!("发送: {} 条", total_send));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("XML: {}", self.xml_path));
            });
        });
    }

    /// 发送测试面板 - 使用columns实现正确的左右分栏
    fn show_send_panel(&mut self, _ctx: &egui::Context, ui: &mut egui::Ui) {
        let available_height = ui.available_height();

        ui.columns(2, |columns| {
            // ==================== 左侧：消息列表 ====================
            columns[0].vertical(|ui| {
                ui.set_min_height(available_height);

                ui.horizontal(|ui| {
                    ui.heading("📋 消息列表");
                    ui.label(format!("(共 {} 条)", self.all_messages.len()));
                });

                // 搜索框
                ui.horizontal(|ui| {
                    ui.label("🔍");
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.search_filter)
                            .hint_text("搜索消息...")
                            .desired_width(ui.available_width() - 30.0),
                    );
                    if ui.button("✖").clicked() || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape))) {
                        self.search_filter.clear();
                    }
                });

                ui.separator();

                // 已选消息（置顶）- 绿色高亮
                if !self.selected_messages.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("✅ 已选消息").strong().color(egui::Color32::from_rgb(50, 200, 50)));
                        ui.label(format!("({})", self.selected_messages.len()));
                    });

                    let selected_height = (self.selected_messages.len() as f32 * 24.0).min(180.0);
                    egui::ScrollArea::vertical()
                        .id_salt("selected_messages")
                        .max_height(selected_height)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            let selected: Vec<_> = self.all_messages
                                .iter()
                                .filter(|(id, _)| self.selected_messages.contains(id))
                                .cloned()
                                .collect();

                            for (msg_id, msg_name) in selected {
                                ui.horizontal(|ui| {
                                    let mut checked = true;
                                    if ui.checkbox(&mut checked, "").changed() && !checked {
                                        self.selected_messages.remove(&msg_id);
                                        self.send_configs.retain(|c| c.msg_id != msg_id);
                                    }
                                    ui.label(egui::RichText::new(format!("[{}]", msg_id)).weak().monospace());
                                    ui.label(&msg_name);
                                    if ui.small_button("✏").on_hover_text("编辑字段").clicked() {
                                        self.open_edit_dialog(msg_id, &msg_name);
                                    }
                                });
                            }
                        });
                    ui.separator();
                }

                // 可选消息列表
                ui.label(egui::RichText::new("📝 可选消息").small().weak());

                egui::ScrollArea::vertical()
                    .id_salt("all_messages")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let filter = self.search_filter.to_lowercase();
                        let mut to_add = Vec::new();

                        for (msg_id, msg_name) in &self.all_messages {
                            if self.selected_messages.contains(msg_id) {
                                continue;
                            }
                            if !filter.is_empty()
                                && !msg_name.to_lowercase().contains(&filter)
                                && !msg_id.to_string().contains(&filter)
                            {
                                continue;
                            }

                            let msg_id = *msg_id;
                            let msg_name = msg_name.clone();

                            ui.horizontal(|ui| {
                                let mut checked = false;
                                if ui.checkbox(&mut checked, "").changed() && checked {
                                    to_add.push((msg_id, msg_name.clone()));
                                }
                                ui.label(egui::RichText::new(format!("[{}]", msg_id)).weak().monospace());
                                ui.label(&msg_name);
                            });
                        }

                        for (msg_id, msg_name) in to_add {
                            self.selected_messages.insert(msg_id);
                            self.add_send_config(msg_id, &msg_name);
                        }
                    });
            });

            // ==================== 右侧：发送配置详情 ====================
            columns[1].vertical(|ui| {
                ui.set_min_height(available_height);

                ui.horizontal(|ui| {
                    ui.heading("⚙ 发送配置");
                    if !self.send_configs.is_empty() {
                        ui.label(format!("({}条)", self.send_configs.len()));

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let enabled_count = self.send_configs.iter().filter(|c| c.enabled).count();
                            if enabled_count > 0 {
                                ui.label(egui::RichText::new(format!("已启用: {}", enabled_count))
                                    .color(egui::Color32::GREEN));
                            }
                        });
                    }
                });

                ui.separator();

                if self.send_configs.is_empty() {
                    ui.add_space(50.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("从左侧选择要发送的消息").size(14.0).weak());
                        ui.add_space(10.0);
                        ui.label("勾选消息后会自动添加到此处");
                        ui.label("点击 ✏ 按钮可编辑字段值");
                    });
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("send_configs")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let mut to_remove = None;
                            let mut to_edit = None;

                            for (idx, config) in self.send_configs.iter_mut().enumerate() {
                                let border_color = if config.enabled {
                                    egui::Color32::from_rgb(0, 180, 0)
                                } else {
                                    ui.style().visuals.widgets.noninteractive.bg_stroke.color
                                };

                                let bg_color = if config.enabled {
                                    egui::Color32::from_rgba_unmultiplied(0, 100, 0, 30)
                                } else {
                                    ui.style().visuals.extreme_bg_color
                                };

                                egui::Frame::none()
                                    .fill(bg_color)
                                    .stroke(egui::Stroke::new(1.5, border_color))
                                    .rounding(6.0)
                                    .inner_margin(10.0)
                                    .outer_margin(egui::Margin::symmetric(0.0, 3.0))
                                    .show(ui, |ui| {
                                        // 第一行：启用开关、名称、按钮
                                        ui.horizontal(|ui| {
                                            ui.checkbox(&mut config.enabled, "");
                                            ui.label(egui::RichText::new(&config.msg_name).strong().size(14.0));
                                            ui.label(egui::RichText::new(format!("[{}]", config.msg_id)).weak().monospace());

                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                if ui.button("🗑").on_hover_text("删除").clicked() {
                                                    to_remove = Some(idx);
                                                }
                                                if ui.button("✏").on_hover_text("编辑字段").clicked() {
                                                    to_edit = Some(idx);
                                                }
                                            });
                                        });

                                        // 第二行：频率和统计
                                        ui.horizontal(|ui| {
                                            ui.label("频率:");
                                            ui.add(
                                                egui::DragValue::new(&mut config.rate_hz)
                                                    .speed(0.1)
                                                    .range(0.1..=100.0)
                                                    .suffix(" Hz"),
                                            );

                                            ui.add_space(20.0);

                                            if let Some(&count) = self.send_stats.get(&config.msg_name) {
                                                ui.label(egui::RichText::new(format!("已发送: {}", count))
                                                    .color(egui::Color32::LIGHT_BLUE));
                                            }
                                        });

                                        // 显示已配置的字段摘要
                                        if !config.fields.is_empty() {
                                            ui.collapsing(format!("字段值 ({})", config.fields.len()), |ui| {
                                                egui::Grid::new(format!("fields_{}", idx))
                                                    .num_columns(2)
                                                    .spacing([10.0, 4.0])
                                                    .show(ui, |ui| {
                                                        for (key, value) in &config.fields {
                                                            ui.label(egui::RichText::new(format!("{}:", key)).weak());
                                                            match value {
                                                                FieldValue::Number(n) => {
                                                                    ui.label(format!("{:.4}", n));
                                                                }
                                                                FieldValue::Text(s) => {
                                                                    ui.label(format!("\"{}\"", s));
                                                                }
                                                                FieldValue::Array(arr) => {
                                                                    let preview: String = arr.iter()
                                                                        .take(4)
                                                                        .map(|v| format!("{:.2}", v))
                                                                        .collect::<Vec<_>>()
                                                                        .join(", ");
                                                                    let suffix = if arr.len() > 4 { "..." } else { "" };
                                                                    ui.label(format!("[{}{}]", preview, suffix));
                                                                }
                                                            }
                                                            ui.end_row();
                                                        }
                                                    });
                                            });
                                        }
                                    });
                            }

                            if let Some(idx) = to_remove {
                                let config = self.send_configs.remove(idx);
                                self.selected_messages.remove(&config.msg_id);
                            }

                            if let Some(idx) = to_edit {
                                let config = &self.send_configs[idx];
                                self.open_edit_dialog_with_config(config.clone(), idx);
                            }
                        });
                }
            });
        });
    }

    /// 接收检测面板 - 使用columns实现正确的左右分栏
    fn show_receive_panel(&mut self, _ctx: &egui::Context, ui: &mut egui::Ui) {
        let available_height = ui.available_height();

        ui.columns(2, |columns| {
            // ==================== 左侧：消息统计列表 ====================
            columns[0].vertical(|ui| {
                ui.set_min_height(available_height);

                ui.horizontal(|ui| {
                    ui.heading("📊 消息统计");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("🗑 清空").clicked() {
                            self.recv_stats.clear();
                            self.recv_messages.clear();
                            self.selected_recv_msg = None;
                        }
                    });
                });

                ui.separator();

                if self.recv_stats.is_empty() {
                    ui.add_space(50.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("等待接收消息...").weak());
                        ui.add_space(10.0);
                        if self.is_connected {
                            ui.label("已连接，等待数据...");
                        } else {
                            ui.label("请先建立连接");
                        }
                    });
                } else {
                    // 先排序，避免每帧都clone和排序
                    let mut indices: Vec<usize> = (0..self.recv_stats.len()).collect();
                    indices.sort_by(|&a, &b| self.recv_stats[b].count.cmp(&self.recv_stats[a].count));

                    egui::ScrollArea::vertical()
                        .id_salt("recv_stats")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let mut new_selection = self.selected_recv_msg;

                            for &idx in &indices {
                                let stat = &self.recv_stats[idx];
                                let is_selected = self.selected_recv_msg == Some(stat.msg_id);

                                let (bg_color, border_color) = if is_selected {
                                    (egui::Color32::from_rgb(40, 60, 100), egui::Color32::from_rgb(80, 140, 200))
                                } else {
                                    (ui.style().visuals.extreme_bg_color, ui.style().visuals.widgets.noninteractive.bg_stroke.color)
                                };

                                let response = egui::Frame::none()
                                    .fill(bg_color)
                                    .stroke(egui::Stroke::new(1.0, border_color))
                                    .rounding(4.0)
                                    .inner_margin(8.0)
                                    .outer_margin(egui::Margin::symmetric(0.0, 2.0))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(&stat.msg_name).strong());
                                            ui.label(egui::RichText::new(format!("[{}]", stat.msg_id)).weak().monospace());
                                        });

                                        ui.horizontal(|ui| {
                                            ui.label(format!("数量: {}", stat.count));
                                            ui.separator();
                                            ui.label(format!("频率: {:.1} Hz", stat.rate_hz));
                                        });

                                        if let Some(header) = &stat.last_header {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "sys:{} comp:{} seq:{}",
                                                    header.system_id, header.component_id, header.sequence
                                                ))
                                                    .small()
                                                    .weak(),
                                            );
                                        }
                                    })
                                    .response;

                                if response.interact(egui::Sense::click()).clicked() {
                                    new_selection = if is_selected { None } else { Some(stat.msg_id) };
                                }
                            }

                            self.selected_recv_msg = new_selection;
                        });
                }
            });

            // ==================== 右侧：选中消息的字段详情 ====================
            columns[1].vertical(|ui| {
                ui.set_min_height(available_height);

                ui.heading("📝 消息详情");
                ui.separator();

                if let Some(selected_id) = self.selected_recv_msg {
                    if let Some(stat) = self.recv_stats.iter().find(|s| s.msg_id == selected_id) {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&stat.msg_name).heading());
                            ui.label(egui::RichText::new(format!("[{}]", stat.msg_id)).weak().monospace());
                        });

                        ui.separator();

                        egui::ScrollArea::vertical()
                            .id_salt("field_details")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                // 显示 header
                                if let Some(header) = &stat.last_header {
                                    ui.collapsing("📌 Header", |ui| {
                                        egui::Grid::new("header_grid")
                                            .num_columns(2)
                                            .spacing([20.0, 4.0])
                                            .show(ui, |ui| {
                                                ui.label("system_id:");
                                                ui.label(format!("{}", header.system_id));
                                                ui.end_row();

                                                ui.label("component_id:");
                                                ui.label(format!("{}", header.component_id));
                                                ui.end_row();

                                                ui.label("sequence:");
                                                ui.label(format!("{}", header.sequence));
                                                ui.end_row();
                                            });
                                    });
                                }

                                ui.add_space(5.0);
                                ui.label(egui::RichText::new("📊 字段值").strong());
                                ui.separator();

                                // 显示字段 - 智能处理char数组
                                // 收集基础字段名（不带[n]后缀的）
                                let mut base_fields: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
                                let mut array_elements: std::collections::HashMap<String, Vec<(usize, f64)>> = std::collections::HashMap::new();

                                for (key, &value) in &stat.last_fields {
                                    let field_name = key.split(':').last().unwrap_or(key);
                                    if let Some(bracket_pos) = field_name.find('[') {
                                        // 数组元素
                                        let base_name = &field_name[..bracket_pos];
                                        if let Some(end_pos) = field_name.find(']') {
                                            if let Ok(idx) = field_name[bracket_pos+1..end_pos].parse::<usize>() {
                                                array_elements
                                                    .entry(base_name.to_string())
                                                    .or_default()
                                                    .push((idx, value));
                                            }
                                        }
                                    } else {
                                        base_fields.insert(field_name.to_string(), value);
                                    }
                                }

                                // 排序数组元素
                                for elements in array_elements.values_mut() {
                                    elements.sort_by_key(|(idx, _)| *idx);
                                }

                                // 收集所有要显示的字段
                                let mut display_fields: Vec<(String, String)> = Vec::new();

                                for (name, value) in &base_fields {
                                    if *value < 0.0 {
                                        // 负数标记表示这是char数组，从数组元素组装字符串
                                        if let Some(elements) = array_elements.get(name) {
                                            let s: String = elements
                                                .iter()
                                                .map(|(_, v)| *v as u8 as char)
                                                .take_while(|&c| c != '\0')
                                                .collect();
                                            display_fields.push((name.clone(), format!("\"{}\"", s)));
                                        }
                                    } else {
                                        display_fields.push((name.clone(), format!("{:.6}", value)));
                                    }
                                }

                                // 对于没有基础字段但有数组元素的情况（非char数组）
                                for (name, elements) in &array_elements {
                                    if !base_fields.contains_key(name) {
                                        let values: Vec<String> = elements
                                            .iter()
                                            .map(|(_, v)| format!("{:.1}", v))
                                            .collect();
                                        display_fields.push((name.clone(), format!("[{}]", values.join(", "))));
                                    }
                                }

                                display_fields.sort_by(|a, b| a.0.cmp(&b.0));

                                egui::Grid::new("fields_grid")
                                    .num_columns(2)
                                    .spacing([20.0, 4.0])
                                    .striped(true)
                                    .show(ui, |ui| {
                                        for (field_name, value_str) in &display_fields {
                                            ui.label(egui::RichText::new(field_name).strong());
                                            ui.label(value_str);
                                            ui.end_row();
                                        }
                                    });
                            });
                    } else {
                        // 消息不存在，清除选中状态（会在下一帧生效）
                        ui.add_space(50.0);
                        ui.vertical_centered(|ui| {
                            ui.label(egui::RichText::new("点击左侧消息查看详情").weak());
                        });
                    }
                } else {
                    ui.add_space(50.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("点击左侧消息查看详情").weak());
                    });
                }
            });
        });
    }

    /// 日志面板
    fn show_log_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("📋 日志");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("🗑 清空").clicked() {
                    self.logs.clear();
                }
            });
        });

        ui.separator();

        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for entry in &self.logs {
                    let color = if entry.is_error {
                        egui::Color32::RED
                    } else {
                        ui.style().visuals.text_color()
                    };

                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&entry.timestamp).weak().small().monospace());
                        ui.label(egui::RichText::new(&entry.message).color(color));
                    });
                }
            });
    }

    /// 显示对话框
    fn show_dialogs(&mut self, ctx: &egui::Context) {
        if self.show_connection_dialog {
            self.show_connection_config_dialog(ctx);
        }

        if self.show_save_dialog {
            self.show_save_record_dialog(ctx);
        }

        if self.show_load_dialog {
            self.show_load_record_dialog(ctx);
        }

        if self.edit_dialog.open {
            self.show_message_edit_dialog(ctx);
        }
    }

    /// 连接配置对话框
    fn show_connection_config_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("🔌 连接配置")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("conn_config_grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("连接类型:");
                        egui::ComboBox::from_id_salt("conn_type")
                            .selected_text(self.connection_config.conn_type.as_str())
                            .show_ui(ui, |ui| {
                                for ct in ConnectionType::all() {
                                    ui.selectable_value(
                                        &mut self.connection_config.conn_type,
                                        ct,
                                        ct.as_str(),
                                    );
                                }
                            });
                        ui.end_row();

                        match self.connection_config.conn_type {
                            ConnectionType::TcpClient | ConnectionType::UdpOut | ConnectionType::Udp => {
                                ui.label("主机:");
                                ui.add(egui::TextEdit::singleline(&mut self.connection_config.host).desired_width(150.0));
                                ui.end_row();

                                ui.label("端口:");
                                ui.add(egui::DragValue::new(&mut self.connection_config.port).range(1..=65535));
                                ui.end_row();

                                if self.connection_config.conn_type == ConnectionType::Udp {
                                    ui.label("本地端口:");
                                    ui.add(egui::DragValue::new(&mut self.connection_config.local_port).range(1..=65535));
                                    ui.end_row();
                                }
                            }
                            ConnectionType::TcpServer | ConnectionType::UdpIn => {
                                ui.label("监听端口:");
                                ui.add(egui::DragValue::new(&mut self.connection_config.port).range(1..=65535));
                                ui.end_row();
                            }
                            ConnectionType::Serial => {
                                ui.label("串口:");
                                ui.horizontal(|ui| {
                                    egui::ComboBox::from_id_salt("serial_port_combo")
                                        .width(120.0)
                                        .selected_text(if self.connection_config.serial_port.is_empty() {
                                            "选择串口".to_string()
                                        } else {
                                            self.connection_config.serial_port.clone()
                                        })
                                        .show_ui(ui, |ui| {
                                            for port in &self.available_ports {
                                                ui.selectable_value(
                                                    &mut self.connection_config.serial_port,
                                                    port.clone(),
                                                    port,
                                                );
                                            }
                                        });
                                    if ui.button("🔄").on_hover_text("刷新串口列表").clicked() {
                                        self.refresh_serial_ports();
                                    }
                                });
                                ui.end_row();

                                ui.label("波特率:");
                                egui::ComboBox::from_id_salt("baud_rate_combo")
                                    .selected_text(format!("{}", self.connection_config.baud_rate))
                                    .show_ui(ui, |ui| {
                                        for &baud in &[9600u32, 19200, 38400, 57600, 115200, 230400, 460800, 921600] {
                                            ui.selectable_value(
                                                &mut self.connection_config.baud_rate,
                                                baud,
                                                format!("{}", baud),
                                            );
                                        }
                                    });
                                ui.end_row();
                            }
                        }

                        ui.label("System ID:");
                        ui.add(egui::DragValue::new(&mut self.connection_config.system_id).range(1..=255));
                        ui.end_row();

                        ui.label("Component ID:");
                        ui.add(egui::DragValue::new(&mut self.connection_config.component_id).range(0..=255));
                        ui.end_row();
                    });

                ui.add_space(15.0);

                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        self.show_connection_dialog = false;
                    }
                    ui.add_space(20.0);
                    if ui.button("连接").clicked() {
                        self.connect();
                        self.show_connection_dialog = false;
                    }
                });
            });
    }

    /// 保存记录对话框
    fn show_save_record_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("💾 保存测试记录")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("记录名称:");
                    ui.add(egui::TextEdit::singleline(&mut self.record_name_input).desired_width(200.0));
                });

                ui.add_space(15.0);

                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        self.show_save_dialog = false;
                    }
                    ui.add_space(20.0);
                    if ui.button("保存").clicked() && !self.record_name_input.is_empty() {
                        let record = SendTestRecord {
                            name: self.record_name_input.clone(),
                            description: String::new(),
                            created_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                            connection: self.connection_config.clone(),
                            messages: self.send_configs.clone(),
                        };
                        match self.config_manager.save_record(&record) {
                            Ok(_) => {
                                self.log(format!("保存成功: {}", record.name));
                                self.saved_records = self.config_manager.list_records();
                            }
                            Err(e) => {
                                self.log_error(format!("保存失败: {}", e));
                            }
                        }
                        self.show_save_dialog = false;
                    }
                });
            });
    }

    /// 加载记录对话框
    fn show_load_record_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("📁 加载测试记录")
            .collapsible(false)
            .resizable(true)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                if self.saved_records.is_empty() {
                    ui.label("没有保存的测试记录");
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .show(ui, |ui| {
                            for name in &self.saved_records.clone() {
                                ui.horizontal(|ui| {
                                    if ui.button(name).clicked() {
                                        match self.config_manager.load_record(name) {
                                            Ok(record) => {
                                                self.connection_config = record.connection;
                                                self.send_configs = record.messages.clone();
                                                self.selected_messages = record.messages.iter().map(|m| m.msg_id).collect();
                                                self.log(format!("加载成功: {}", name));
                                            }
                                            Err(e) => {
                                                self.log_error(format!("加载失败: {}", e));
                                            }
                                        }
                                        self.show_load_dialog = false;
                                    }
                                    if ui.small_button("🗑").clicked() {
                                        let _ = self.config_manager.delete_record(name);
                                        self.saved_records = self.config_manager.list_records();
                                    }
                                });
                            }
                        });
                }

                ui.add_space(15.0);

                if ui.button("关闭").clicked() {
                    self.show_load_dialog = false;
                }
            });
    }

    /// 消息编辑对话框
    fn show_message_edit_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.edit_dialog.open;

        egui::Window::new(format!("✏ 编辑消息: {}", self.edit_dialog.config.msg_name))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .min_width(500.0)
            .min_height(400.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                // Header 设置
                ui.collapsing("📌 Header 设置", |ui| {
                    ui.checkbox(&mut self.edit_dialog.config.use_custom_header, "使用自定义 Header");
                    if self.edit_dialog.config.use_custom_header {
                        ui.horizontal(|ui| {
                            ui.label("System ID:");
                            ui.add(egui::DragValue::new(&mut self.edit_dialog.config.header_system_id).range(1..=255));
                            ui.add_space(20.0);
                            ui.label("Component ID:");
                            ui.add(egui::DragValue::new(&mut self.edit_dialog.config.header_component_id).range(0..=255));
                        });
                    }
                });

                // 发送频率
                ui.horizontal(|ui| {
                    ui.label("发送频率:");
                    ui.add(
                        egui::DragValue::new(&mut self.edit_dialog.config.rate_hz)
                            .speed(0.1)
                            .range(0.1..=100.0)
                            .suffix(" Hz"),
                    );
                });

                ui.separator();
                ui.label(egui::RichText::new("📊 字段值").strong());

                // 字段编辑
                egui::ScrollArea::vertical()
                    .max_height(350.0)
                    .show(ui, |ui| {
                        if let Some(mapper) = &self.mapper {
                            if let Some(msg_def) = mapper.get_message_def(self.edit_dialog.config.msg_id) {
                                let field_info = mapper.get_sorted_field_info_with_enum(msg_def);

                                egui::Grid::new("field_edit_grid")
                                    .num_columns(2)
                                    .spacing([10.0, 6.0])
                                    .show(ui, |ui| {
                                        for (field_name, field_type, units, enum_type, is_ext, _offset) in field_info {
                                            let label = if is_ext {
                                                format!("{}* ({}):", field_name, units)
                                            } else if !units.is_empty() {
                                                format!("{} ({}):", field_name, units)
                                            } else {
                                                format!("{}:", field_name)
                                            };

                                            ui.label(egui::RichText::new(label).strong());

                                            let value_str = self.edit_dialog.field_values
                                                .entry(field_name.clone())
                                                .or_insert_with(|| {
                                                    if let Some(v) = self.edit_dialog.config.fields.get(&field_name) {
                                                        match v {
                                                            FieldValue::Number(n) => format!("{}", n),
                                                            FieldValue::Text(s) => s.clone(),
                                                            FieldValue::Array(arr) => {
                                                                if field_type.is_char_array() {
                                                                    // 对于char数组，将数字转换为字符串
                                                                    arr.iter()
                                                                        .map(|&v| v as u8 as char)
                                                                        .take_while(|&c| c != '\0')
                                                                        .collect()
                                                                } else {
                                                                    arr.iter()
                                                                        .map(|v| format!("{}", v))
                                                                        .collect::<Vec<_>>()
                                                                        .join(",")
                                                                }
                                                            }
                                                        }
                                                    } else if field_type.is_char_array() {
                                                        String::new()  // char数组默认空字符串
                                                    } else {
                                                        "0".to_string()
                                                    }
                                                });

                                            if field_type.is_char_array() {
                                                // char数组：使用字符串输入
                                                let len = field_type.array_length();
                                                ui.add(
                                                    egui::TextEdit::singleline(value_str)
                                                        .desired_width(220.0)
                                                        .hint_text(format!("字符串，最长 {} 字符", len)),
                                                );
                                            } else if field_type.is_array() {
                                                let len = field_type.array_length();
                                                ui.add(
                                                    egui::TextEdit::singleline(value_str)
                                                        .desired_width(220.0)
                                                        .hint_text(format!("{} 个值，逗号分隔", len)),
                                                );
                                            } else if let Some(enum_name) = &enum_type {
                                                if let Some(enum_def) = mapper.get_enum_def(enum_name) {
                                                    egui::ComboBox::from_id_salt(format!("enum_{}", field_name))
                                                        .width(220.0)
                                                        .selected_text(value_str.as_str())
                                                        .show_ui(ui, |ui| {
                                                            for entry in &enum_def.entries {
                                                                if ui.selectable_label(
                                                                    *value_str == entry.value.to_string(),
                                                                    format!("{} ({})", entry.name, entry.value),
                                                                ).clicked() {
                                                                    *value_str = entry.value.to_string();
                                                                }
                                                            }
                                                        });
                                                } else {
                                                    ui.add(egui::TextEdit::singleline(value_str).desired_width(150.0));
                                                }
                                            } else {
                                                ui.add(egui::TextEdit::singleline(value_str).desired_width(150.0));
                                            }

                                            ui.end_row();
                                        }
                                    });
                            }
                        }
                    });

                ui.separator();

                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        self.edit_dialog.open = false;
                    }
                    ui.add_space(20.0);
                    if ui.button("保存").clicked() {
                        self.apply_edit_dialog();
                        self.edit_dialog.open = false;
                    }
                });
            });

        self.edit_dialog.open = open;
    }

    // ========== 辅助方法 ==========

    fn add_send_config(&mut self, msg_id: u32, msg_name: &str) {
        let config = SendMessageConfig {
            id: uuid::Uuid::new_v4().to_string(),
            msg_name: msg_name.to_string(),
            msg_id,
            enabled: false,
            rate_hz: 1.0,
            fields: HashMap::new(),
            use_custom_header: false,
            header_system_id: 255,
            header_component_id: 0,
        };
        self.send_configs.push(config);
    }

    fn open_edit_dialog(&mut self, msg_id: u32, msg_name: &str) {
        if let Some(idx) = self.send_configs.iter().position(|c| c.msg_id == msg_id) {
            self.open_edit_dialog_with_config(self.send_configs[idx].clone(), idx);
        } else {
            let config = SendMessageConfig {
                id: uuid::Uuid::new_v4().to_string(),
                msg_name: msg_name.to_string(),
                msg_id,
                enabled: false,
                rate_hz: 1.0,
                fields: HashMap::new(),
                use_custom_header: false,
                header_system_id: 255,
                header_component_id: 0,
            };
            self.edit_dialog.config = config;
            self.edit_dialog.field_values.clear();
            self.edit_dialog.editing_index = None;
            self.edit_dialog.open = true;
        }
    }

    fn open_edit_dialog_with_config(&mut self, config: SendMessageConfig, idx: usize) {
        self.edit_dialog.config = config;
        self.edit_dialog.field_values.clear();
        self.edit_dialog.editing_index = Some(idx);
        self.edit_dialog.open = true;
    }

    fn apply_edit_dialog(&mut self) {
        for (field_name, value_str) in &self.edit_dialog.field_values {
            let value_str = value_str.trim();
            if value_str.is_empty() {
                continue;
            }

            if value_str.contains(',') {
                let values: Vec<f64> = value_str
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                if !values.is_empty() {
                    self.edit_dialog.config.fields.insert(
                        field_name.clone(),
                        FieldValue::Array(values),
                    );
                }
            } else if let Ok(n) = value_str.parse::<f64>() {
                self.edit_dialog.config.fields.insert(
                    field_name.clone(),
                    FieldValue::Number(n),
                );
            } else {
                self.edit_dialog.config.fields.insert(
                    field_name.clone(),
                    FieldValue::Text(value_str.to_string()),
                );
            }
        }

        if let Some(idx) = self.edit_dialog.editing_index {
            if idx < self.send_configs.len() {
                self.send_configs[idx] = self.edit_dialog.config.clone();
            }
        } else {
            self.send_configs.push(self.edit_dialog.config.clone());
            self.selected_messages.insert(self.edit_dialog.config.msg_id);
        }
    }
}