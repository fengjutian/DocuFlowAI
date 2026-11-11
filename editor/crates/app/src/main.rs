use anyhow::Result;
use codeweaver_platform::NativeApplication;

fn main() -> Result<()> {
    NativeApplication::default().run()
}
