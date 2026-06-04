/// Miku Tunes 应用配置
pub struct AppConfig;

impl AppConfig {
    /// 应用名称
    pub const NAME: &'static str = "Miku Tunes";
    /// 应用中文描述
    pub const NAME_ZH: &'static str = "基于 Tauri + Rust 的轻量级桌面音乐播放器";
    /// 版本号
    pub const VERSION: &'static str = "1.0";
    /// GitHub 仓库地址
    pub const GITHUB_URL: &'static str = "https://github.com/your-username/miku-tunes";
    /// 项目简介
    pub const DESCRIPTION: &'static str = "一款以初音未来（Hatsune Miku）为核心视觉IP的轻量级桌面音乐播放器，采用Tauri + Rust技术栈构建。";
    /// 作者
    pub const AUTHOR: &'static str = "Miku Tunes Team";
}
