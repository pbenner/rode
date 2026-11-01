//! Stream Python helpers through stdin, then exec a short, self-removing script.
//! Exact unbuffered reads leave coalesced protocol bytes available to the helper.
use anyhow::{Result, ensure};

const LIMIT: usize = 4 * 1024 * 1024;
// SSH keeps this compact first stage in its own argv. The staging logic and
// complete helper source travel over stdin instead.
const LOADER: &str = r#"import os
def r(n):
 b=bytearray()
 while len(b)<n:
  p=os.read(0,n-len(b))
  if not p: raise EOFError('Incomplete RODE helper')
  b.extend(p)
 return b
n=int.from_bytes(r(4),'big')
if not 0<n<=4194304: raise ValueError('RODE helper exceeds 4 MiB')
exec(compile(r(n),'<rode-loader>','exec'))"#;

pub(crate) const PROCESS: Script = Script {
    name: "rode-ssh-process.py",
    source: include_str!("../remote_agent.py"),
};
pub(crate) const HOST: Script = Script {
    name: "rode-host.py",
    source: crate::files::REMOTE_HELPER,
};

pub(crate) struct Script {
    name: &'static str,
    source: &'static str,
}
impl Script {
    pub fn command(&self) -> String {
        format!(
            "python3 -u -c {} {}",
            super::quote(LOADER),
            super::quote(self.name)
        )
    }
    /// Prepend only code, never request data, to the script's original stdin.
    pub fn input(&self, input: Vec<u8>) -> Result<Vec<u8>> {
        let stage = format!(
            "script_name={}\nscript_source={}\n{}",
            serde_json::to_string(self.name)?,
            serde_json::to_string(self.source)?,
            include_str!("python_stage.py"),
        );
        ensure!(stage.len() <= LIMIT, "RODE helper exceeds 4 MiB");
        let mut framed = Vec::with_capacity(4 + stage.len() + input.len());
        framed.extend_from_slice(&(stage.len() as u32).to_be_bytes());
        framed.extend_from_slice(stage.as_bytes());
        framed.extend(input);
        Ok(framed)
    }
}

#[cfg(test)]
mod tests;
