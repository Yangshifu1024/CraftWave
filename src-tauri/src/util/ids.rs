//! id 生成（§2.2 util/ids）：ProviderConfig.id / ProviderModel.id 均 uuid（§3.2）。

/// 新 uuid v4（小写连字符形式）。
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}
