use std::net::IpAddr;

#[derive(Clone, Debug)]
pub struct IpWhitelist {
    patterns: Vec<String>,
}

impl IpWhitelist {
    pub fn new(patterns: Vec<String>) -> Self {
        Self { patterns }
    }

    pub fn is_allowed(&self, ip: IpAddr) -> bool {
        if self.patterns.is_empty() {
            return true;
        }

        let ip = ip.to_string();
        self.patterns.iter().any(|pattern| glob_match(pattern, &ip))
    }

    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }
}

fn glob_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut p, mut v) = (0usize, 0usize);
    let mut star = None;
    let mut match_v = 0usize;

    while v < value.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == value[v]) {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            match_v = v;
        } else if let Some(star_pos) = star {
            p = star_pos + 1;
            match_v += 1;
            v = match_v;
        } else {
            return false;
        }
    }

    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }

    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn supports_exact_match() {
        assert!(glob_match("192.168.1.10", "192.168.1.10"));
        assert!(!glob_match("192.168.1.10", "192.168.1.11"));
    }

    #[test]
    fn supports_wildcard_match() {
        assert!(glob_match("192.168.1.*", "192.168.1.10"));
        assert!(glob_match("10.0.?.5", "10.0.1.5"));
        assert!(!glob_match("10.0.?.5", "10.0.10.5"));
    }

    #[test]
    fn supports_ipv6_match() {
        assert!(glob_match("2001:db8::*", "2001:db8::1"));
        assert!(!glob_match("2001:db9::*", "2001:db8::1"));
    }
}
