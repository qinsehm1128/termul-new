//! Compile with --crate-type=cdylib -C panic=abort and an --extern pointing to
//! se_plugin_abi: export_plugin must fail with the panic=unwind diagnostic.
use se_plugin_abi::{export_plugin, Host, PluginV1, SeStatus};

struct AbortFixture;
impl PluginV1 for AbortFixture {
    fn manifest() -> &'static [u8] {
        b"{}"
    }
    fn create(_: Host, _: &[u8]) -> Result<Self, SeStatus> {
        Ok(Self)
    }
}
export_plugin!(AbortFixture);
