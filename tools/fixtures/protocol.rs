// Minimal, independent closure exercising every structural mapping supported by the generator.
// protocol.golden was captured from the Python generator, with only the Run header updated.
#[serde(rename_all = "camelCase")]
pub struct ExternalRuntimeRequest<'a> {
    pub request_id: u64,
    pub text: &'a str,
    pub optional: Option<String>,
    pub nested: BTreeMap<String, Vec<Option<u8>>>,
    pub handler: WireRuntimeHandler,
    pub action: WireWindowAction,
    pub decoration: WireDecorationChild,
    pub dimension: WireDimension,
    pub family: WireFontFamily,
    pub click: WireOnClick,
    pub resize: WireResizeHitArea,
    pub effect: WireCompiledEffect,
    pub animation: ManagedWindowAnimationSnapshot,
    pub json: serde_json::Value,
    #[serde(rename = "switch")]
    pub switch_value: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<u8>>,
}

#[serde(default, rename_all = "camelCase")]
pub struct ExternalRuntimeResponse {
    pub text: String,
    pub list: Vec<u8>,
    pub map: BTreeMap<String, WireDecorationNode>,
    pub node: WireDecorationNode,
    pub flag: bool,
    pub signed: i32,
    pub unsigned: u32,
    pub big: u64,
    pub byte: u8,
    pub single: f32,
    pub double: f64,
    pub json: serde_json::Value,
    pub optional: Option<WireDecorationNode>,
}

#[serde(rename_all = "kebab-case")]
pub enum WireRuntimeHandler {
    OnClick,
    #[serde(rename = "xdg-decoration-v1")]
    Decoration,
}

#[serde(rename_all = "lowercase")]
pub enum WireWindowAction {
    Close,
    Maximize,
}

pub struct WireDecorationNode {
    #[serde(default)]
    pub children: Vec<WireDecorationChild>,
    pub optional: Option<bool>,
}
