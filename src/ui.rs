// ============================================================================
// MAVLink 测试台 GUI - 老年模式优化版
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

// ============================================================================
// 老年模式常量配置
// ============================================================================

/// 基础字体大小
const FONT_SIZE_BASE: f32 = 16.0;
/// 标题字体大小
const FONT_SIZE_HEADING: f32 = 20.0;
/// 大标题字体大小
const FONT_SIZE_LARGE: f32 = 18.0;
/// 小字体大小
const FONT_SIZE_SMALL: f32 = 14.0;
/// 按钮最小高度
const BUTTON_MIN_HEIGHT: f32 = 32.0;
/// 大按钮最小高度
const BUTTON_LARGE_HEIGHT: f32 = 36.0;
/// 控件间距
const SPACING_NORMAL: f32 = 8.0;
/// 大间距
const SPACING_LARGE: f32 = 15.0;

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
    window_id: u8,
    cmd_tx: Sender<UiCommand>,
    event_rx: Receiver<BackendEvent>,
    config_manager: ConfigManager,
    current_record: SendTestRecord,
    saved_records: Vec<String>,
    record_name_input: String,
    mapper: Option<Arc<MavMapper>>,
    xml_path: String,
    is_connected: bool,
    is_connecting: bool,
    is_sending: bool,
    connection_config: ConnectionConfig,
    connection_id: u64,
    available_ports: Vec<String>,
    all_messages: Vec<(u32, String)>,
    search_filter: String,
    selected_messages: HashSet<u32>,
    send_configs: Vec<SendMessageConfig>,
    edit_dialog: MessageEditDialog,
    recv_stats: Vec<MessageStats>,
    recv_messages: Vec<ReceivedMessage>,
    selected_recv_msg: Option<u32>,
    logs: Vec<LogEntry>,
    active_tab: ActiveTab,
    show_connection_dialog: bool,
    show_save_dialog: bool,
    show_load_dialog: bool,
    send_stats: HashMap<String, u64>,
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

        // 设置全局样式 - 老年模式
        let mut style = (*cc.egui_ctx.style()).clone();

        // 增大默认字体
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(FONT_SIZE_BASE, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(FONT_SIZE_BASE, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(FONT_SIZE_HEADING, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(FONT_SIZE_BASE, egui::FontFamily::Monospace),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(FONT_SIZE_SMALL, egui::FontFamily::Proportional),
        );

        // 增大控件间距
        style.spacing.item_spacing = egui::vec2(SPACING_NORMAL, SPACING_NORMAL);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);

        cc.egui_ctx.set_style(style);

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

        if !app_config.xml_path.is_empty() {
            app.load_xml(&app_config.xml_path);
        }

        app
    }

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
                    if event_conn_id != self.connection_id {
                        continue;
                    }
                    if connected {
                        if self.is_connecting {
                            self.is_connected = true;
                            self.is_connecting = false;
                            self.log("已连接".to_string());
                            // TCP连接成功后自动发送
                            let enabled_count = self.send_configs.iter().filter(|c| c.enabled).count();
                            if enabled_count > 0 {
                                self.start_sending();
                            }
                        }
                    } else {
                        self.is_connected = false;
                        self.is_connecting = false;
                        self.is_sending = false;
                        self.log("已断开".to_string());
                    }
                }
                BackendEvent::MessageReceived(header, msg_id, msg_name, fields) => {
                    if !self.is_connected && !self.is_connecting {
                        continue;
                    }
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
                    if !self.is_connected {
                        continue;
                    }
                    for new_stat in stats {
                        if let Some(existing) = self.recv_stats.iter_mut().find(|s| s.msg_id == new_stat.msg_id) {
                            existing.count = new_stat.count;
                            existing.rate_hz = new_stat.rate_hz;
                            existing.last_seen = new_stat.last_seen;
                            existing.last_header = new_stat.last_header;
                            existing.last_fields = new_stat.last_fields;
                        } else {
                            self.recv_stats.push(new_stat);
                        }
                    }
                }
                BackendEvent::Log(msg) => self.log(msg),
                BackendEvent::Error(msg) => self.log_error(msg),
                BackendEvent::SendStats { msg_name, count } => {
                    self.send_stats.insert(msg_name, count);
                }
            }
        }

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
        self.connection_id += 1;
        self.is_connecting = true;
        while self.event_rx.try_recv().is_ok() {}
        let _ = self.cmd_tx.send(UiCommand::Connect(self.connection_config.clone(), self.connection_id));

        // UDP/串口：确定后立即开始发送（不需要等待连接确认）
        match self.connection_config.conn_type {
            ConnectionType::Udp | ConnectionType::UdpIn | ConnectionType::UdpOut | ConnectionType::Serial => {
                self.is_connected = true;
                self.is_connecting = false;
                let enabled_count = self.send_configs.iter().filter(|c| c.enabled).count();
                if enabled_count > 0 {
                    self.start_sending();
                }
            }
            _ => {
                // TCP：等待连接成功事件
            }
        }
    }

    fn disconnect(&mut self) {
        self.is_connecting = false;
        self.is_connected = false;
        self.is_sending = false;
        self.recv_stats.clear();
        self.recv_messages.clear();
        self.selected_recv_msg = None;
        while self.event_rx.try_recv().is_ok() {}
        let _ = self.cmd_tx.send(UiCommand::Disconnect);
    }

    fn start_sending(&mut self) {
        let configs: Vec<_> = self.send_configs.iter().filter(|c| c.enabled).cloned().collect();
        if configs.is_empty() {
            // 静默处理，不再报错（因为可能是自动调用）
            return;
        }
        let _ = self.cmd_tx.send(UiCommand::StartSending(configs));
        self.is_sending = true;
    }

    fn stop_sending(&mut self) {
        let _ = self.cmd_tx.send(UiCommand::StopSending);
        self.is_sending = false;
    }

    /// 发送配置更新到后端（实时生效）
    fn send_config_update(&self) {
        if self.is_sending {
            let configs = self.send_configs.clone();
            let _ = self.cmd_tx.send(UiCommand::UpdateSendConfig(configs));
        }
    }
}

impl eframe::App for MavTestbedApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_events();
        ctx.request_repaint_after(Duration::from_millis(100));

        egui::TopBottomPanel::top("top_panel")
            .min_height(50.0)
            .show(ctx, |ui| {
                self.show_top_bar(ui);
            });

        egui::TopBottomPanel::bottom("bottom_panel")
            .min_height(36.0)
            .show(ctx, |ui| {
                self.show_status_bar(ui);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(SPACING_NORMAL);

            // 标签页选择 - 增大按钮
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = SPACING_LARGE;

                let tab_btn = |ui: &mut egui::Ui, current: &mut ActiveTab, target: ActiveTab, icon: &str, text: &str| {
                    let is_selected = *current == target;
                    let btn_text = egui::RichText::new(format!("{} {}", icon, text))
                        .size(FONT_SIZE_LARGE);

                    let btn = egui::Button::new(btn_text)
                        .min_size(egui::vec2(120.0, BUTTON_LARGE_HEIGHT))
                        .selected(is_selected);

                    if ui.add(btn).clicked() {
                        *current = target;
                    }
                };

                tab_btn(ui, &mut self.active_tab, ActiveTab::Send, "📤", "发送测试");
                tab_btn(ui, &mut self.active_tab, ActiveTab::Receive, "📥", "接收检测");
                tab_btn(ui, &mut self.active_tab, ActiveTab::Log, "📋", "运行日志");
            });

            ui.add_space(SPACING_NORMAL);
            ui.separator();
            ui.add_space(SPACING_NORMAL);

            match self.active_tab {
                ActiveTab::Send => self.show_send_panel(ctx, ui),
                ActiveTab::Receive => self.show_receive_panel(ctx, ui),
                ActiveTab::Log => self.show_log_panel(ui),
            }
        });

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
    /// 创建带图标和文字的大按钮
    fn large_button(ui: &mut egui::Ui, icon: &str, text: &str) -> egui::Response {
        ui.add(
            egui::Button::new(
                egui::RichText::new(format!("{} {}", icon, text)).size(FONT_SIZE_BASE)
            )
                .min_size(egui::vec2(0.0, BUTTON_MIN_HEIGHT))
        )
    }

    /// 创建带图标和文字的小按钮
    fn medium_button(ui: &mut egui::Ui, icon: &str, text: &str) -> egui::Response {
        ui.add(
            egui::Button::new(
                egui::RichText::new(format!("{} {}", icon, text)).size(FONT_SIZE_SMALL)
            )
                .min_size(egui::vec2(0.0, 28.0))
        )
    }

    /// 顶部工具栏
    fn show_top_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SPACING_LARGE;

            ui.label(
                egui::RichText::new(format!("🛩 MAVLink测试台 #{}", self.window_id))
                    .strong()
                    .size(FONT_SIZE_HEADING),
            );

            ui.separator();

            // XML加载
            if Self::large_button(ui, "📂", "加载XML").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("XML", &["xml"])
                    .pick_file()
                {
                    self.load_xml(&path.to_string_lossy());
                }
            }

            ui.label(
                egui::RichText::new(format!("消息: {}", self.all_messages.len()))
                    .size(FONT_SIZE_BASE)
            );

            ui.separator();

            // 连接控制
            if self.is_connected {
                let addr = self.connection_config.to_addr_string();
                ui.label(
                    egui::RichText::new("● 已连接")
                        .color(egui::Color32::GREEN)
                        .size(FONT_SIZE_BASE)
                );
                ui.label(
                    egui::RichText::new(format!("[{}]", addr))
                        .monospace()
                        .size(FONT_SIZE_SMALL)
                );
                if Self::large_button(ui, "⛓", "断开连接").clicked() {
                    self.disconnect();
                }
            } else if self.is_connecting {
                let addr = self.connection_config.to_addr_string();
                ui.label(
                    egui::RichText::new("● 连接中...")
                        .color(egui::Color32::YELLOW)
                        .size(FONT_SIZE_BASE)
                );
                ui.label(
                    egui::RichText::new(format!("[{}]", addr))
                        .monospace()
                        .size(FONT_SIZE_SMALL)
                );
                if Self::large_button(ui, "✖", "取消").clicked() {
                    self.disconnect();
                }
            } else {
                ui.label(
                    egui::RichText::new("○ 未连接")
                        .color(egui::Color32::GRAY)
                        .size(FONT_SIZE_BASE)
                );
                if Self::large_button(ui, "🔌", "连接设备").clicked() {
                    self.show_connection_dialog = true;
                }
            }

            ui.separator();

            // 发送控制
            if self.is_connected {
                if !self.is_sending {
                    if Self::large_button(ui, "▶", "开始发送").clicked() {
                        self.start_sending();
                    }
                } else {
                    if Self::large_button(ui, "⏹", "停止发送").clicked() {
                        self.stop_sending();
                    }
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = SPACING_NORMAL;
                if Self::large_button(ui, "💾", "保存配置").clicked() {
                    self.show_save_dialog = true;
                }
                if Self::large_button(ui, "📁", "加载配置").clicked() {
                    self.saved_records = self.config_manager.list_records();
                    self.show_load_dialog = true;
                }
            });
        });
        ui.add_space(4.0);
    }

    /// 底部状态栏
    fn show_status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SPACING_LARGE;

            let recv_count = self.recv_stats.len();
            ui.label(
                egui::RichText::new(format!("📥 接收: {} 种消息", recv_count))
                    .size(FONT_SIZE_BASE)
            );

            ui.separator();

            let total_recv: u64 = self.recv_stats.iter().map(|s| s.count).sum();
            ui.label(
                egui::RichText::new(format!("总计: {} 条", total_recv))
                    .size(FONT_SIZE_BASE)
            );

            ui.separator();

            let total_send: u64 = self.send_stats.values().sum();
            ui.label(
                egui::RichText::new(format!("📤 发送: {} 条", total_send))
                    .size(FONT_SIZE_BASE)
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("📄 {}", self.xml_path))
                        .size(FONT_SIZE_SMALL)
                );
            });
        });
    }

    /// 发送测试面板
    fn show_send_panel(&mut self, _ctx: &egui::Context, ui: &mut egui::Ui) {
        let available_height = ui.available_height();

        ui.columns(2, |columns| {
            // ==================== 左侧：消息列表 ====================
            columns[0].vertical(|ui| {
                ui.set_min_height(available_height);

                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("📋 消息列表")
                            .size(FONT_SIZE_LARGE)
                            .strong()
                    );
                    ui.label(
                        egui::RichText::new(format!("(共 {} 条)", self.all_messages.len()))
                            .size(FONT_SIZE_BASE)
                    );
                });

                ui.add_space(SPACING_NORMAL);

                // 搜索框
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("🔍 搜索:").size(FONT_SIZE_BASE));
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.search_filter)
                            .hint_text("输入消息名称或ID...")
                            .desired_width(ui.available_width() - 80.0)
                            .font(egui::FontId::new(FONT_SIZE_BASE, egui::FontFamily::Proportional)),
                    );
                    if Self::medium_button(ui, "✖", "清除").clicked()
                        || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)))
                    {
                        self.search_filter.clear();
                    }
                });

                ui.add_space(SPACING_NORMAL);
                ui.separator();
                ui.add_space(SPACING_NORMAL);

                // 已选消息（置顶）
                if !self.selected_messages.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("✅ 已选消息")
                                .strong()
                                .color(egui::Color32::from_rgb(50, 200, 50))
                                .size(FONT_SIZE_BASE)
                        );
                        ui.label(
                            egui::RichText::new(format!("({})", self.selected_messages.len()))
                                .size(FONT_SIZE_BASE)
                        );
                    });

                    ui.add_space(SPACING_NORMAL);

                    let selected_height = (self.selected_messages.len() as f32 * 32.0).min(200.0);
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
                                    let cb = egui::Checkbox::new(&mut checked, "");
                                    if ui.add(cb).changed() && !checked {
                                        self.selected_messages.remove(&msg_id);
                                        self.send_configs.retain(|c| c.msg_id != msg_id);
                                        self.send_config_update();
                                    }
                                    ui.label(
                                        egui::RichText::new(format!("[{}]", msg_id))
                                            .weak()
                                            .monospace()
                                            .size(FONT_SIZE_BASE)
                                    );
                                    ui.label(
                                        egui::RichText::new(&msg_name).size(FONT_SIZE_BASE)
                                    );
                                    if Self::medium_button(ui, "✏", "编辑").clicked() {
                                        self.open_edit_dialog(msg_id, &msg_name);
                                    }
                                });
                            }
                        });

                    ui.add_space(SPACING_NORMAL);
                    ui.separator();
                    ui.add_space(SPACING_NORMAL);
                }

                // 可选消息列表
                ui.label(
                    egui::RichText::new("📝 可选消息")
                        .size(FONT_SIZE_SMALL)
                        .weak()
                );

                ui.add_space(SPACING_NORMAL);

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
                                ui.label(
                                    egui::RichText::new(format!("[{}]", msg_id))
                                        .weak()
                                        .monospace()
                                        .size(FONT_SIZE_BASE)
                                );
                                ui.label(
                                    egui::RichText::new(&msg_name).size(FONT_SIZE_BASE)
                                );
                            });
                        }

                        for (msg_id, msg_name) in to_add {
                            self.selected_messages.insert(msg_id);
                            self.add_send_config(msg_id, &msg_name);
                            self.send_config_update();
                        }
                    });
            });

            // ==================== 右侧：发送配置详情 ====================
            columns[1].vertical(|ui| {
                ui.set_min_height(available_height);

                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("⚙ 发送配置")
                            .size(FONT_SIZE_LARGE)
                            .strong()
                    );
                    if !self.send_configs.is_empty() {
                        ui.label(
                            egui::RichText::new(format!("({}条)", self.send_configs.len()))
                                .size(FONT_SIZE_BASE)
                        );

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let enabled_count = self.send_configs.iter().filter(|c| c.enabled).count();
                            if enabled_count > 0 {
                                ui.label(
                                    egui::RichText::new(format!("✓ 已启用: {}", enabled_count))
                                        .color(egui::Color32::GREEN)
                                        .size(FONT_SIZE_BASE)
                                );
                            }
                        });
                    }
                });

                ui.add_space(SPACING_NORMAL);
                ui.separator();
                ui.add_space(SPACING_NORMAL);

                if self.send_configs.is_empty() {
                    ui.add_space(60.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("从左侧选择要发送的消息")
                                .size(FONT_SIZE_BASE)
                                .weak()
                        );
                    });
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("send_configs")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let mut to_remove = None;
                            let mut to_edit = None;
                            let mut config_changed = false;
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
                                    .stroke(egui::Stroke::new(2.0, border_color))
                                    .rounding(8.0)
                                    .inner_margin(12.0)
                                    .outer_margin(egui::Margin::symmetric(0.0, 4.0))
                                    .show(ui, |ui| {
                                        // 第一行：启用开关、名称、按钮
                                        ui.horizontal(|ui| {
                                            if ui.checkbox(&mut config.enabled, "").changed() && self.is_sending {
                                                config_changed = true;
                                            }
                                            ui.label(
                                                egui::RichText::new(&config.msg_name)
                                                    .strong()
                                                    .size(FONT_SIZE_LARGE)
                                            );
                                            ui.label(
                                                egui::RichText::new(format!("[{}]", config.msg_id))
                                                    .weak()
                                                    .monospace()
                                                    .size(FONT_SIZE_BASE)
                                            );

                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                if Self::medium_button(ui, "🗑", "删除").clicked() {
                                                    to_remove = Some(idx);
                                                }
                                                if Self::medium_button(ui, "✏", "编辑").clicked() {
                                                    to_edit = Some(idx);
                                                }
                                            });
                                        });

                                        ui.add_space(SPACING_NORMAL);

                                        // 第二行：频率和统计
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("发送频率:").size(FONT_SIZE_BASE)
                                            );
                                            if ui.add(
                                                egui::DragValue::new(&mut config.rate_hz)
                                                    .speed(0.1)
                                                    .range(0.1..=100.0)
                                                    .suffix(" Hz"),
                                            ).changed() && self.is_sending {
                                                config_changed = true;
                                            }

                                            ui.add_space(SPACING_LARGE);

                                            if let Some(&count) = self.send_stats.get(&config.msg_name) {
                                                ui.label(
                                                    egui::RichText::new(format!("已发送: {}", count))
                                                        .color(egui::Color32::LIGHT_BLUE)
                                                        .size(FONT_SIZE_BASE)
                                                );
                                            }
                                        });

                                        // 显示已配置的字段摘要
                                        if !config.fields.is_empty() {
                                            ui.add_space(SPACING_NORMAL);
                                            ui.collapsing(
                                                egui::RichText::new(format!("📊 字段值 ({})", config.fields.len()))
                                                    .size(FONT_SIZE_BASE),
                                                |ui| {
                                                    egui::Grid::new(format!("fields_{}", idx))
                                                        .num_columns(2)
                                                        .spacing([SPACING_LARGE, SPACING_NORMAL])
                                                        .show(ui, |ui| {
                                                            for (key, value) in &config.fields {
                                                                ui.label(
                                                                    egui::RichText::new(format!("{}:", key))
                                                                        .weak()
                                                                        .size(FONT_SIZE_BASE)
                                                                );
                                                                match value {
                                                                    FieldValue::Number(n) => {
                                                                        ui.label(
                                                                            egui::RichText::new(format!("{:.4}", n))
                                                                                .size(FONT_SIZE_BASE)
                                                                        );
                                                                    }
                                                                    FieldValue::Text(s) => {
                                                                        ui.label(
                                                                            egui::RichText::new(format!("\"{}\"", s))
                                                                                .size(FONT_SIZE_BASE)
                                                                        );
                                                                    }
                                                                    FieldValue::Array(arr) => {
                                                                        let preview: String = arr.iter()
                                                                            .take(4)
                                                                            .map(|v| format!("{:.2}", v))
                                                                            .collect::<Vec<_>>()
                                                                            .join(", ");
                                                                        let suffix = if arr.len() > 4 { "..." } else { "" };
                                                                        ui.label(
                                                                            egui::RichText::new(format!("[{}{}]", preview, suffix))
                                                                                .size(FONT_SIZE_BASE)
                                                                        );
                                                                    }
                                                                }
                                                                ui.end_row();
                                                            }
                                                        });
                                                }
                                            );
                                        }
                                    });
                            }
                            if config_changed {
                                self.send_config_update();
                            }
                            if let Some(idx) = to_remove {
                                let config = self.send_configs.remove(idx);
                                self.selected_messages.remove(&config.msg_id);
                                self.send_config_update();
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

    /// 接收检测面板
    fn show_receive_panel(&mut self, _ctx: &egui::Context, ui: &mut egui::Ui) {
        let available_height = ui.available_height();

        ui.columns(2, |columns| {
            // ==================== 左侧：消息统计列表 ====================
            columns[0].vertical(|ui| {
                ui.set_min_height(available_height);

                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("📊 消息统计")
                            .size(FONT_SIZE_LARGE)
                            .strong()
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if Self::medium_button(ui, "🗑", "清空").clicked() {
                            self.recv_stats.clear();
                            self.recv_messages.clear();
                            self.selected_recv_msg = None;
                        }
                    });
                });

                ui.add_space(SPACING_NORMAL);
                ui.separator();
                ui.add_space(SPACING_NORMAL);

                if self.recv_stats.is_empty() {
                    ui.add_space(60.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("等待接收消息...")
                                .weak()
                                .size(FONT_SIZE_LARGE)
                        );
                        ui.add_space(SPACING_LARGE);
                        if self.is_connected {
                            ui.label(
                                egui::RichText::new("已连接，等待数据...")
                                    .size(FONT_SIZE_BASE)
                            );
                        } else {
                            ui.label(
                                egui::RichText::new("请先建立连接")
                                    .size(FONT_SIZE_BASE)
                            );
                        }
                    });
                } else {
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
                                    .stroke(egui::Stroke::new(1.5, border_color))
                                    .rounding(6.0)
                                    .inner_margin(10.0)
                                    .outer_margin(egui::Margin::symmetric(0.0, 3.0))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(&stat.msg_name)
                                                    .strong()
                                                    .size(FONT_SIZE_BASE)
                                            );
                                            ui.label(
                                                egui::RichText::new(format!("[{}]", stat.msg_id))
                                                    .weak()
                                                    .monospace()
                                                    .size(FONT_SIZE_BASE)
                                            );
                                        });

                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(format!("数量: {}", stat.count))
                                                    .size(FONT_SIZE_BASE)
                                            );
                                            ui.separator();
                                            ui.label(
                                                egui::RichText::new(format!("频率: {:.1} Hz", stat.rate_hz))
                                                    .size(FONT_SIZE_BASE)
                                            );
                                        });

                                        if let Some(header) = &stat.last_header {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "sys:{} comp:{} seq:{}",
                                                    header.system_id, header.component_id, header.sequence
                                                ))
                                                    .size(FONT_SIZE_SMALL)
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

                ui.label(
                    egui::RichText::new("🔍 消息详情")
                        .size(FONT_SIZE_LARGE)
                        .strong()
                );

                ui.add_space(SPACING_NORMAL);
                ui.separator();
                ui.add_space(SPACING_NORMAL);

                if let Some(selected_id) = self.selected_recv_msg {
                    if let Some(stat) = self.recv_stats.iter().find(|s| s.msg_id == selected_id) {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&stat.msg_name)
                                    .size(FONT_SIZE_HEADING)
                                    .strong()
                            );
                            ui.label(
                                egui::RichText::new(format!("[{}]", stat.msg_id))
                                    .weak()
                                    .monospace()
                                    .size(FONT_SIZE_BASE)
                            );
                        });

                        ui.add_space(SPACING_NORMAL);
                        ui.separator();
                        ui.add_space(SPACING_NORMAL);

                        egui::ScrollArea::vertical()
                            .id_salt("field_details")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                // 显示 header
                                if let Some(header) = &stat.last_header {
                                    ui.collapsing(
                                        egui::RichText::new("📌 Header 信息").size(FONT_SIZE_BASE),
                                        |ui| {
                                            egui::Grid::new("header_grid")
                                                .num_columns(2)
                                                .spacing([SPACING_LARGE, SPACING_NORMAL])
                                                .show(ui, |ui| {
                                                    ui.label(egui::RichText::new("system_id:").size(FONT_SIZE_BASE));
                                                    ui.label(egui::RichText::new(format!("{}", header.system_id)).size(FONT_SIZE_BASE));
                                                    ui.end_row();

                                                    ui.label(egui::RichText::new("component_id:").size(FONT_SIZE_BASE));
                                                    ui.label(egui::RichText::new(format!("{}", header.component_id)).size(FONT_SIZE_BASE));
                                                    ui.end_row();

                                                    ui.label(egui::RichText::new("sequence:").size(FONT_SIZE_BASE));
                                                    ui.label(egui::RichText::new(format!("{}", header.sequence)).size(FONT_SIZE_BASE));
                                                    ui.end_row();
                                                });
                                        }
                                    );
                                }

                                ui.add_space(SPACING_NORMAL);
                                ui.label(
                                    egui::RichText::new("📊 字段值")
                                        .strong()
                                        .size(FONT_SIZE_BASE)
                                );
                                ui.separator();
                                ui.add_space(SPACING_NORMAL);

                                // 显示字段
                                let mut base_fields: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
                                let mut array_elements: std::collections::HashMap<String, Vec<(usize, f64)>> = std::collections::HashMap::new();

                                for (key, &value) in &stat.last_fields {
                                    let field_name = key.split(':').last().unwrap_or(key);
                                    if let Some(bracket_pos) = field_name.find('[') {
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

                                for elements in array_elements.values_mut() {
                                    elements.sort_by_key(|(idx, _)| *idx);
                                }

                                let mut display_fields: Vec<(String, String)> = Vec::new();

                                for (name, value) in &base_fields {
                                    if *value < 0.0 {
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
                                    .spacing([SPACING_LARGE, SPACING_NORMAL])
                                    .striped(true)
                                    .show(ui, |ui| {
                                        for (field_name, value_str) in &display_fields {
                                            ui.label(
                                                egui::RichText::new(field_name)
                                                    .strong()
                                                    .size(FONT_SIZE_BASE)
                                            );
                                            ui.label(
                                                egui::RichText::new(value_str)
                                                    .size(FONT_SIZE_BASE)
                                            );
                                            ui.end_row();
                                        }
                                    });
                            });
                    } else {
                        ui.add_space(60.0);
                        ui.vertical_centered(|ui| {
                            ui.label(
                                egui::RichText::new("点击左侧消息查看详情")
                                    .weak()
                                    .size(FONT_SIZE_LARGE)
                            );
                        });
                    }
                } else {
                    ui.add_space(60.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("点击左侧消息查看详情")
                                .weak()
                                .size(FONT_SIZE_LARGE)
                        );
                    });
                }
            });
        });
    }

    /// 日志面板
    fn show_log_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("📋 运行日志")
                    .size(FONT_SIZE_LARGE)
                    .strong()
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if Self::medium_button(ui, "🗑", "清空日志").clicked() {
                    self.logs.clear();
                }
            });
        });

        ui.add_space(SPACING_NORMAL);
        ui.separator();
        ui.add_space(SPACING_NORMAL);

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
                        ui.label(
                            egui::RichText::new(&entry.timestamp)
                                .weak()
                                .monospace()
                                .size(FONT_SIZE_BASE)
                        );
                        ui.label(
                            egui::RichText::new(&entry.message)
                                .color(color)
                                .size(FONT_SIZE_BASE)
                        );
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
        egui::Window::new(egui::RichText::new("🔌 连接配置").size(FONT_SIZE_HEADING))
            .collapsible(false)
            .resizable(false)
            .min_width(400.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(SPACING_NORMAL);

                egui::Grid::new("conn_config_grid")
                    .num_columns(2)
                    .spacing([SPACING_LARGE, SPACING_LARGE])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("连接类型:").size(FONT_SIZE_BASE));
                        egui::ComboBox::from_id_salt("conn_type")
                            .width(200.0)
                            .selected_text(egui::RichText::new(self.connection_config.conn_type.as_str()).size(FONT_SIZE_BASE))
                            .show_ui(ui, |ui| {
                                for ct in ConnectionType::all() {
                                    ui.selectable_value(
                                        &mut self.connection_config.conn_type,
                                        ct,
                                        egui::RichText::new(ct.as_str()).size(FONT_SIZE_BASE),
                                    );
                                }
                            });
                        ui.end_row();

                        match self.connection_config.conn_type {
                            ConnectionType::TcpClient | ConnectionType::UdpOut | ConnectionType::Udp => {
                                ui.label(egui::RichText::new("主机地址:").size(FONT_SIZE_BASE));
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.connection_config.host)
                                        .desired_width(200.0)
                                        .font(egui::FontId::new(FONT_SIZE_BASE, egui::FontFamily::Proportional))
                                );
                                ui.end_row();

                                ui.label(egui::RichText::new("端口号:").size(FONT_SIZE_BASE));
                                ui.add(egui::DragValue::new(&mut self.connection_config.port).range(1..=65535));
                                ui.end_row();

                                if self.connection_config.conn_type == ConnectionType::Udp {
                                    ui.label(egui::RichText::new("本地端口:").size(FONT_SIZE_BASE));
                                    ui.add(egui::DragValue::new(&mut self.connection_config.local_port).range(1..=65535));
                                    ui.end_row();
                                }
                            }
                            ConnectionType::TcpServer | ConnectionType::UdpIn => {
                                ui.label(egui::RichText::new("监听端口:").size(FONT_SIZE_BASE));
                                ui.add(egui::DragValue::new(&mut self.connection_config.port).range(1..=65535));
                                ui.end_row();
                            }
                            ConnectionType::Serial => {
                                ui.label(egui::RichText::new("串口:").size(FONT_SIZE_BASE));
                                ui.horizontal(|ui| {
                                    egui::ComboBox::from_id_salt("serial_port_combo")
                                        .width(150.0)
                                        .selected_text(if self.connection_config.serial_port.is_empty() {
                                            egui::RichText::new("选择串口").size(FONT_SIZE_BASE)
                                        } else {
                                            egui::RichText::new(&self.connection_config.serial_port).size(FONT_SIZE_BASE)
                                        })
                                        .show_ui(ui, |ui| {
                                            for port in &self.available_ports {
                                                ui.selectable_value(
                                                    &mut self.connection_config.serial_port,
                                                    port.clone(),
                                                    egui::RichText::new(port).size(FONT_SIZE_BASE),
                                                );
                                            }
                                        });
                                    if Self::medium_button(ui, "🔄", "刷新").clicked() {
                                        self.refresh_serial_ports();
                                    }
                                });
                                ui.end_row();

                                ui.label(egui::RichText::new("波特率:").size(FONT_SIZE_BASE));
                                egui::ComboBox::from_id_salt("baud_rate_combo")
                                    .width(150.0)
                                    .selected_text(egui::RichText::new(format!("{}", self.connection_config.baud_rate)).size(FONT_SIZE_BASE))
                                    .show_ui(ui, |ui| {
                                        for &baud in &[9600u32, 19200, 38400, 57600, 115200, 230400, 460800, 921600] {
                                            ui.selectable_value(
                                                &mut self.connection_config.baud_rate,
                                                baud,
                                                egui::RichText::new(format!("{}", baud)).size(FONT_SIZE_BASE),
                                            );
                                        }
                                    });
                                ui.end_row();
                            }
                        }

                        ui.label(egui::RichText::new("System ID:").size(FONT_SIZE_BASE));
                        ui.add(egui::DragValue::new(&mut self.connection_config.system_id).range(1..=255));
                        ui.end_row();

                        ui.label(egui::RichText::new("Component ID:").size(FONT_SIZE_BASE));
                        ui.add(egui::DragValue::new(&mut self.connection_config.component_id).range(0..=255));
                        ui.end_row();
                    });

                ui.add_space(SPACING_LARGE);

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = SPACING_LARGE;
                    if Self::large_button(ui, "✖", "取消").clicked() {
                        self.show_connection_dialog = false;
                    }
                    if Self::large_button(ui, "✔", "确认连接").clicked() {
                        self.connect();
                        self.show_connection_dialog = false;
                    }
                });
            });
    }

    /// 保存记录对话框
    fn show_save_record_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new(egui::RichText::new("💾 保存测试记录").size(FONT_SIZE_HEADING))
            .collapsible(false)
            .resizable(false)
            .min_width(350.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(SPACING_NORMAL);

                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("记录名称:").size(FONT_SIZE_BASE));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.record_name_input)
                            .desired_width(220.0)
                            .font(egui::FontId::new(FONT_SIZE_BASE, egui::FontFamily::Proportional))
                    );
                });

                ui.add_space(SPACING_LARGE);

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = SPACING_LARGE;
                    if Self::large_button(ui, "✖", "取消").clicked() {
                        self.show_save_dialog = false;
                    }
                    if Self::large_button(ui, "✔", "保存").clicked() && !self.record_name_input.is_empty() {
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
        egui::Window::new(egui::RichText::new("📁 加载测试记录").size(FONT_SIZE_HEADING))
            .collapsible(false)
            .resizable(true)
            .min_width(350.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(SPACING_NORMAL);

                if self.saved_records.is_empty() {
                    ui.label(egui::RichText::new("没有保存的测试记录").size(FONT_SIZE_BASE));
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(350.0)
                        .show(ui, |ui| {
                            for name in &self.saved_records.clone() {
                                ui.horizontal(|ui| {
                                    if ui.add(
                                        egui::Button::new(egui::RichText::new(name).size(FONT_SIZE_BASE))
                                            .min_size(egui::vec2(200.0, BUTTON_MIN_HEIGHT))
                                    ).clicked() {
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
                                    if Self::medium_button(ui, "🗑", "删除").clicked() {
                                        let _ = self.config_manager.delete_record(name);
                                        self.saved_records = self.config_manager.list_records();
                                    }
                                });
                            }
                        });
                }

                ui.add_space(SPACING_LARGE);

                if Self::large_button(ui, "✖", "关闭").clicked() {
                    self.show_load_dialog = false;
                }
            });
    }

    /// 消息编辑对话框
    fn show_message_edit_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.edit_dialog.open;
        let mut should_close = false;

        egui::Window::new(
            egui::RichText::new(format!("✏ 编辑消息: {}", self.edit_dialog.config.msg_name))
                .size(FONT_SIZE_HEADING)
        )
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .min_width(550.0)
            .min_height(450.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                // Header 设置
                ui.collapsing(
                    egui::RichText::new("📌 Header 设置").size(FONT_SIZE_BASE),
                    |ui| {
                        ui.checkbox(
                            &mut self.edit_dialog.config.use_custom_header,
                            egui::RichText::new("使用自定义 Header").size(FONT_SIZE_BASE)
                        );
                        if self.edit_dialog.config.use_custom_header {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("System ID:").size(FONT_SIZE_BASE));
                                ui.add(egui::DragValue::new(&mut self.edit_dialog.config.header_system_id).range(1..=255));
                                ui.add_space(SPACING_LARGE);
                                ui.label(egui::RichText::new("Component ID:").size(FONT_SIZE_BASE));
                                ui.add(egui::DragValue::new(&mut self.edit_dialog.config.header_component_id).range(0..=255));
                            });
                        }
                    }
                );

                ui.add_space(SPACING_NORMAL);

                // 发送频率
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("发送频率:").size(FONT_SIZE_BASE));
                    ui.add(
                        egui::DragValue::new(&mut self.edit_dialog.config.rate_hz)
                            .speed(0.1)
                            .range(0.1..=100.0)
                            .suffix(" Hz"),
                    );
                });

                ui.add_space(SPACING_NORMAL);
                ui.separator();
                ui.add_space(SPACING_NORMAL);

                ui.label(egui::RichText::new("📊 字段值").strong().size(FONT_SIZE_BASE));

                // 字段编辑
                egui::ScrollArea::vertical()
                    .max_height(400.0)
                    .show(ui, |ui| {
                        if let Some(mapper) = &self.mapper {
                            if let Some(msg_def) = mapper.get_message_def(self.edit_dialog.config.msg_id) {
                                let field_info = mapper.get_sorted_field_info_with_enum(msg_def);

                                egui::Grid::new("field_edit_grid")
                                    .num_columns(2)
                                    .spacing([SPACING_LARGE, SPACING_NORMAL])
                                    .show(ui, |ui| {
                                        for (field_name, field_type, units, enum_type, is_ext, _offset) in field_info {
                                            let label = if is_ext {
                                                format!("{}* ({}):", field_name, units)
                                            } else if !units.is_empty() {
                                                format!("{} ({}):", field_name, units)
                                            } else {
                                                format!("{}:", field_name)
                                            };

                                            ui.label(egui::RichText::new(label).strong().size(FONT_SIZE_BASE));

                                            let value_str = self.edit_dialog.field_values
                                                .entry(field_name.clone())
                                                .or_insert_with(|| {
                                                    if let Some(v) = self.edit_dialog.config.fields.get(&field_name) {
                                                        match v {
                                                            FieldValue::Number(n) => format!("{}", n),
                                                            FieldValue::Text(s) => s.clone(),
                                                            FieldValue::Array(arr) => {
                                                                if field_type.is_char_array() {
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
                                                        String::new()
                                                    } else {
                                                        "0".to_string()
                                                    }
                                                });

                                            if field_type.is_char_array() {
                                                let len = field_type.array_length();
                                                ui.add(
                                                    egui::TextEdit::singleline(value_str)
                                                        .desired_width(250.0)
                                                        .hint_text(format!("字符串，最长 {} 字符", len))
                                                        .font(egui::FontId::new(FONT_SIZE_BASE, egui::FontFamily::Proportional)),
                                                );
                                            } else if field_type.is_array() {
                                                let len = field_type.array_length();
                                                ui.add(
                                                    egui::TextEdit::singleline(value_str)
                                                        .desired_width(250.0)
                                                        .hint_text(format!("{} 个值，逗号分隔", len))
                                                        .font(egui::FontId::new(FONT_SIZE_BASE, egui::FontFamily::Proportional)),
                                                );
                                            } else if let Some(enum_name) = &enum_type {
                                                if let Some(enum_def) = mapper.get_enum_def(enum_name) {
                                                    egui::ComboBox::from_id_salt(format!("enum_{}", field_name))
                                                        .width(250.0)
                                                        .selected_text(egui::RichText::new(value_str.as_str()).size(FONT_SIZE_BASE))
                                                        .show_ui(ui, |ui| {
                                                            for entry in &enum_def.entries {
                                                                if ui.selectable_label(
                                                                    *value_str == entry.value.to_string(),
                                                                    egui::RichText::new(format!("{} ({})", entry.name, entry.value)).size(FONT_SIZE_BASE),
                                                                ).clicked() {
                                                                    *value_str = entry.value.to_string();
                                                                }
                                                            }
                                                        });
                                                } else {
                                                    ui.add(
                                                        egui::TextEdit::singleline(value_str)
                                                            .desired_width(180.0)
                                                            .font(egui::FontId::new(FONT_SIZE_BASE, egui::FontFamily::Proportional))
                                                    );
                                                }
                                            } else {
                                                ui.add(
                                                    egui::TextEdit::singleline(value_str)
                                                        .desired_width(180.0)
                                                        .font(egui::FontId::new(FONT_SIZE_BASE, egui::FontFamily::Proportional))
                                                );
                                            }

                                            ui.end_row();
                                        }
                                    });
                            }
                        }
                    });

                ui.add_space(SPACING_NORMAL);
                ui.separator();
                ui.add_space(SPACING_NORMAL);

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = SPACING_LARGE;
                    if Self::large_button(ui, "✖", "取消").clicked() {
                        should_close = true;
                    }
                    if Self::large_button(ui, "✔", "保存").clicked() {
                        self.apply_edit_dialog();
                        should_close = true;
                    }
                });
            });

        // 窗口X按钮或我们的按钮都可以关闭
        self.edit_dialog.open = open && !should_close;
    }

    // ========== 辅助方法 ==========

    fn add_send_config(&mut self, msg_id: u32, msg_name: &str) {
        let config = SendMessageConfig {
            id: uuid::Uuid::new_v4().to_string(),
            msg_name: msg_name.to_string(),
            msg_id,
            enabled: true,  // 勾选即启用
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

        // 实时更新到后端
        self.send_config_update();
    }
}