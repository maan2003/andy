use std::fmt;
use std::process;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow, bail};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacAddress(pub [u8; 6]);

impl MacAddress {
    pub fn generated() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let mut hash = nanos ^ u128::from(process::id());
        let mut bytes = [0u8; 6];
        for byte in &mut bytes {
            *byte = hash as u8;
            hash >>= 8;
        }

        // Locally administered, unicast.
        bytes[0] = (bytes[0] | 0x02) & 0xfe;
        Self(bytes)
    }

    pub fn validate(self) -> Result<Self> {
        if self.0 == [0; 6] {
            bail!("MAC address must not be all zeros");
        }
        if self.0[0] & 0x01 != 0 {
            bail!("MAC address must be unicast");
        }
        Ok(self)
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

impl FromStr for MacAddress {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0u8; 6];
        let mut parts = s.split(':');
        for byte in &mut bytes {
            let part = parts.next().ok_or_else(|| anyhow!("invalid MAC address"))?;
            *byte = u8::from_str_radix(part, 16)
                .map_err(|_| anyhow!("invalid MAC address component `{part}`"))?;
        }
        if parts.next().is_some() {
            bail!("invalid MAC address");
        }
        MacAddress(bytes).validate()
    }
}
