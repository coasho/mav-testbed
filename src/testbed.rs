// ============================================================================
// 测试台后端逻辑
// ============================================================================

use crate::config::{ConnectionConfig, FieldValue, SendMessageConfig};
use crate::mav_conn::{MavConfig, MavRx, MavTx, connect};
use crossbeam_channel::{Receiver, Sender};
use mavlink::{MavHeader, Message};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};
use crate::mav_mapper::MavMapper;

/// 接收到的消息统计
#[derive(Debug, Clone, Default)]
pub struct MessageStats {
    pub msg_id: u32,
    pub msg_name: String,
    pub count: u64,
    pub rate_hz: f32,
    pub last_seen: Option<Instant>,
    pub last_header: Option<MavHeader>,
    pub last_fields: HashMap<String, f64>,
}

/// 后端事件
#[derive(Debug, Clone)]
pub enum BackendEvent {
    ConnectionStateChanged(bool, u64),
    MessageReceived(MavHeader, u32, String, HashMap<String, f64>),
    StatsUpdated(Vec<MessageStats>),
    Log(String),
    Error(String),
    SendStats { msg_name: String, count: u64 },
}

/// UI命令
#[derive(Debug, Clone)]
pub enum UiCommand {
    Connect(ConnectionConfig, u64),
    Disconnect,
    StartSending(Vec<SendMessageConfig>),
    StopSending,
    UpdateSendConfig(Vec<SendMessageConfig>),
    LoadXml(String),
    Shutdown,
}

/// 共享的发送配置
pub type SharedSendConfigs = Arc<RwLock<Vec<SendMessageConfig>>>;

/// 测试台后端
pub struct TestbedBackend {
    event_tx: Sender<BackendEvent>,
    cmd_rx: Receiver<UiCommand>,
    mapper: Option<Arc<MavMapper>>,
    running: Arc<AtomicBool>,
    send_configs: SharedSendConfigs,  // 共享配置
}

impl TestbedBackend {
    pub fn new(event_tx: Sender<BackendEvent>, cmd_rx: Receiver<UiCommand>) -> Self {
        Self {
            event_tx,
            cmd_rx,
            mapper: None,
            running: Arc::new(AtomicBool::new(true)),
            send_configs: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn run(&mut self) {
        let mut mav_tx: Option<MavTx> = None;
        let mut connection_running = Arc::new(AtomicBool::new(false));
        let mut send_running = Arc::new(AtomicBool::new(false));
        let mut stats: HashMap<u32, MessageStats> = HashMap::new();
        let mut last_stats_update = Instant::now();
        let mut current_connection_id: u64 = 0;

        while self.running.load(Ordering::Relaxed) {
            while let Ok(cmd) = self.cmd_rx.try_recv() {
                match cmd {
                    UiCommand::LoadXml(path) => {
                        self.log(format!("加载XML: {}", path));
                        match MavMapper::new(&path) {
                            Ok(mapper) => {
                                self.mapper = Some(Arc::new(mapper));
                                self.log(format!("XML加载成功: {}", path));
                            }
                            Err(e) => {
                                self.error(format!("XML加载失败: {}", e));
                            }
                        }
                    }

                    UiCommand::Connect(config, conn_id) => {
                        connection_running.store(false, Ordering::Relaxed);
                        send_running.store(false, Ordering::Relaxed);
                        if let Some(ref tx) = mav_tx {
                            tx.shutdown();
                        }
                        mav_tx = None;
                        stats.clear();

                        let addr = config.to_addr_string();
                        let is_connectionless = config.conn_type.is_connectionless();
                        self.log(format!("连接中: {}", addr));

                        let mav_config = MavConfig::new("testbed", &addr, &addr)
                            .with_self_id(config.system_id, config.component_id)
                            .with_heartbeat(1000)
                            .with_subscriptions(vec![]);

                        let (tx, rx) = connect(mav_config);
                        mav_tx = Some(tx);
                        current_connection_id = conn_id;

                        let event_tx = self.event_tx.clone();
                        let mapper = self.mapper.clone();
                        let conn_running = Arc::new(AtomicBool::new(true));
                        connection_running = conn_running.clone();

                        thread::Builder::new()
                            .name("mav-recv".to_string())
                            .spawn(move || {
                                Self::recv_loop(rx, event_tx, mapper, conn_running, is_connectionless, conn_id);
                            })
                            .expect("spawn recv thread");
                    }

                    UiCommand::Disconnect => {
                        self.log("断开连接".to_string());
                        connection_running.store(false, Ordering::Relaxed);
                        send_running.store(false, Ordering::Relaxed);
                        if let Some(ref tx) = mav_tx {
                            tx.shutdown();
                        }
                        mav_tx = None;
                        stats.clear();
                        let _ = self.event_tx.send(BackendEvent::ConnectionStateChanged(false, current_connection_id));
                    }

                    UiCommand::StartSending(configs) => {
                        if let Some(tx) = mav_tx.clone() {
                            if let Some(mapper) = self.mapper.clone() {
                                // 更新共享配置
                                if let Ok(mut cfg) = self.send_configs.write() {
                                    *cfg = configs;
                                }

                                send_running.store(true, Ordering::Relaxed);
                                let running = send_running.clone();
                                let event_tx = self.event_tx.clone();
                                let shared_configs = self.send_configs.clone();

                                thread::Builder::new()
                                    .name("mav-send".to_string())
                                    .spawn(move || {
                                        Self::send_loop(tx, mapper, shared_configs, running, event_tx);
                                    })
                                    .expect("spawn send thread");

                                self.log("开始发送".to_string());
                            } else {
                                self.error("未加载XML，无法发送".to_string());
                            }
                        } else {
                            self.error("未连接，无法发送".to_string());
                        }
                    }

                    UiCommand::StopSending => {
                        send_running.store(false, Ordering::Relaxed);
                        self.log("停止发送".to_string());
                    }

                    UiCommand::UpdateSendConfig(configs) => {
                        // 实时更新共享配置
                        if let Ok(mut cfg) = self.send_configs.write() {
                            *cfg = configs;
                        }
                    }

                    UiCommand::Shutdown => {
                        self.running.store(false, Ordering::Relaxed);
                        connection_running.store(false, Ordering::Relaxed);
                        send_running.store(false, Ordering::Relaxed);
                        break;
                    }
                }
            }

            if last_stats_update.elapsed() > Duration::from_millis(1000) {
                let stats_vec: Vec<MessageStats> = stats.values().cloned().collect();
                let _ = self.event_tx.send(BackendEvent::StatsUpdated(stats_vec));
                last_stats_update = Instant::now();
            }

            thread::sleep(Duration::from_millis(10));
        }
    }

    fn recv_loop(
        rx: MavRx,
        event_tx: Sender<BackendEvent>,
        mapper: Option<Arc<MavMapper>>,
        running: Arc<AtomicBool>,
        is_connectionless: bool,
        connection_id: u64,
    ) {
        let mut stats: HashMap<u32, MessageStats> = HashMap::new();
        let mut last_stats_send = Instant::now();
        let mut connection_reported = false;

        if is_connectionless {
            let _ = event_tx.send(BackendEvent::ConnectionStateChanged(true, connection_id));
            connection_reported = true;
        }

        while running.load(Ordering::Relaxed) {
            if !is_connectionless {
                let is_connected = rx.is_connected();

                if is_connected && !connection_reported {
                    connection_reported = true;
                    let _ = event_tx.send(BackendEvent::ConnectionStateChanged(true, connection_id));
                }

                if connection_reported && !is_connected {
                    rx.shutdown();
                    let _ = event_tx.send(BackendEvent::ConnectionStateChanged(false, connection_id));
                    let _ = event_tx.send(BackendEvent::Log("对端已断开".to_string()));
                    break;
                }
            }

            if let Some((header, msg)) = rx.recv_timeout(Duration::from_millis(100)) {
                if !running.load(Ordering::Relaxed) {
                    break;
                }

                if !connection_reported {
                    connection_reported = true;
                    let _ = event_tx.send(BackendEvent::ConnectionStateChanged(true, connection_id));
                }

                let msg_id = msg.message_id();
                let msg_name = mapper
                    .as_ref()
                    .and_then(|m| m.get_message_name(msg_id))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("MSG_{}", msg_id));

                let mut fields = HashMap::new();
                if let Some(mapper) = &mapper {
                    mapper.parsing_mavlink_msg(&msg, &mut fields);
                }

                let stat = stats.entry(msg_id).or_insert_with(|| MessageStats {
                    msg_id,
                    msg_name: msg_name.clone(),
                    ..Default::default()
                });
                stat.count += 1;
                let now = Instant::now();
                if let Some(last) = stat.last_seen {
                    let elapsed = now.duration_since(last).as_secs_f32();
                    if elapsed > 0.001 {
                        stat.rate_hz = stat.rate_hz * 0.9 + (1.0 / elapsed) * 0.1;
                    }
                }
                stat.last_seen = Some(now);
                stat.last_header = Some(header);
                stat.last_fields = fields.clone();

                let _ = event_tx.send(BackendEvent::MessageReceived(
                    header,
                    msg_id,
                    msg_name,
                    fields,
                ));

                if last_stats_send.elapsed() > Duration::from_millis(1000) {
                    let stats_vec: Vec<MessageStats> = stats.values().cloned().collect();
                    let _ = event_tx.send(BackendEvent::StatsUpdated(stats_vec));
                    last_stats_send = now;
                }
            }
        }
    }

    fn send_loop(
        tx: MavTx,
        mapper: Arc<MavMapper>,
        shared_configs: SharedSendConfigs,
        running: Arc<AtomicBool>,
        event_tx: Sender<BackendEvent>,
    ) {
        let mut last_send: HashMap<String, Instant> = HashMap::new();
        let mut send_counts: HashMap<String, u64> = HashMap::new();

        let _ = event_tx.send(BackendEvent::Log("发送线程启动".to_string()));

        while running.load(Ordering::Relaxed) {
            let now = Instant::now();

            // 每次循环读取最新配置
            let configs = match shared_configs.read() {
                Ok(cfg) => cfg.clone(),
                Err(_) => continue,
            };

            for config in &configs {
                if !config.enabled || config.rate_hz <= 0.0 {
                    continue;
                }

                let interval = Duration::from_secs_f32(1.0 / config.rate_hz);
                let should_send = last_send
                    .get(&config.id)
                    .map(|t| now.duration_since(*t) >= interval)
                    .unwrap_or(true);

                if !should_send {
                    continue;
                }

                // 构建消息字段
                let mut metas: HashMap<String, f64> = HashMap::new();
                for (key, value) in &config.fields {
                    let full_key = format!("{}:{}", config.msg_name, key);
                    match value {
                        FieldValue::Number(n) => {
                            metas.insert(full_key, *n);
                        }
                        FieldValue::Array(arr) => {
                            for (i, v) in arr.iter().enumerate() {
                                let arr_key = format!("{}:{}[{}]", config.msg_name, key, i);
                                metas.insert(arr_key, *v);
                            }
                        }
                        FieldValue::Text(s) => {
                            for (i, b) in s.bytes().enumerate() {
                                let char_key = format!("{}:{}[{}]", config.msg_name, key, i);
                                metas.insert(char_key, b as f64);
                            }
                        }
                    }
                }

                let (target_sys, target_comp) = tx.target();

                match mapper.get_mavlink_msg_with_target(
                    config.msg_id,
                    &metas,
                    target_sys,
                    target_comp,
                ) {
                    Some(msg) => {
                        let result = if config.use_custom_header {
                            let header = MavHeader {
                                system_id: config.header_system_id,
                                component_id: config.header_component_id,
                                sequence: 0,
                            };
                            tx.send_with_header(header, msg)
                        } else {
                            tx.send(msg)
                        };

                        if let Err(e) = result {
                            let _ = event_tx.send(BackendEvent::Error(format!("发送失败: {} - {}", config.msg_name, e)));
                        }

                        last_send.insert(config.id.clone(), now);
                        *send_counts.entry(config.msg_name.clone()).or_insert(0) += 1;

                        let _ = event_tx.send(BackendEvent::SendStats {
                            msg_name: config.msg_name.clone(),
                            count: send_counts[&config.msg_name],
                        });
                    }
                    None => {
                        if !last_send.contains_key(&config.id) {
                            let _ = event_tx.send(BackendEvent::Error(format!("消息构建失败: {} (id={})", config.msg_name, config.msg_id)));
                        }
                    }
                }
            }

            thread::sleep(Duration::from_millis(1));
        }
    }

    fn log(&self, msg: String) {
        let _ = self.event_tx.send(BackendEvent::Log(msg));
    }

    fn error(&self, msg: String) {
        let _ = self.event_tx.send(BackendEvent::Error(msg));
    }
}