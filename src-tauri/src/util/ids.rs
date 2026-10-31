//! id 生成（util/ids）：ProviderConfig.id / ProviderModel.id 均 uuid。

/// 新 uuid v4（小写连字符形式，如 "3b241101-e2bb-4255-8caf-4136c566a962"）。
/// v4 是纯随机版本，碰撞概率可忽略，用作配置项/模型等对象的全局唯一 id。
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}
