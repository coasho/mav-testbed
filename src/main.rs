// ============================================================================
// MAVLink 测试台 - 主入口
// ============================================================================

// Windows下隐藏控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_assignments)]
#![allow(unused_mut)]
mod config;
mod mavlink;
mod mav_mapper;
mod mav_conn;
mod testbed;
mod ui;

use eframe::egui;
use crate::mav_conn::InstanceLock;

fn main() -> eframe::Result<()> {
    // 获取实例锁（支持多窗口，自动分配ID）
    let instance_lock = InstanceLock::acquire("mav_testbed")
        .expect("无法获取实例锁");
    let window_id = instance_lock.id;
    match embed_dir::embed_dir!("mavlink_xml") {
        Ok(true) => println!("已解压 config/host 目录"),
        Ok(false) => println!("config/host 目录已存在，跳过解压"),
        Err(e) => eprintln!("解压失败: {}", e),
    }
    println!("╔════════════════════════════════════════╗");
    println!("║     MAVLink 测试台 - 窗口 #{}          ║", window_id);
    println!("╚════════════════════════════════════════╝");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([800.0, 600.0])
            .with_title(format!("MAVLink调试工具 #{}", window_id)),
        ..Default::default()
    };

    eframe::run_native(
        &"MAVLink调试工具".to_string(),
        options,
        Box::new(move |cc| {
            Ok(Box::new(ui::MavTestbedApp::new(cc, window_id)))
        }),
    )
}
