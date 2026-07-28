//! Test helpers: DNS → combined rust/ts name bags (emit uses `naming::*`).

use crate::error::IdlError;
use crate::naming::rust::RustNames;
use crate::naming::typescript::TsNames;

/// Temporary helper used by unit tests until callers move to naming modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Idents {
    pub rust_field: String,
    pub rust_variant: String,
    pub rust_const: String,
    pub ts_prop: String,
    pub ts_type: String,
    pub ts_const: String,
}

pub fn remap_dns(dns: &str) -> Result<Idents, IdlError> {
    let rust = RustNames::from_dns(dns)?;
    let ts = TsNames::from_dns(dns)?;
    Ok(Idents {
        rust_field: rust.field,
        rust_variant: rust.variant,
        rust_const: rust.const_name,
        ts_prop: ts.prop,
        ts_type: ts.type_name,
        ts_const: ts.const_name,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn remap_web() {
        let idents = remap_dns("web").unwrap();
        assert_eq!(idents.rust_field, "web");
        assert_eq!(idents.rust_variant, "Web");
        assert_eq!(idents.rust_const, "WEB");
        assert_eq!(idents.ts_prop, "web");
        assert_eq!(idents.ts_type, "Web");
        assert_eq!(idents.ts_const, "WEB");
    }

    #[test]
    fn remap_my_api() {
        let idents = remap_dns("my-api").unwrap();
        assert_eq!(idents.rust_field, "my_api");
        assert_eq!(idents.rust_variant, "MyApi");
        assert_eq!(idents.rust_const, "MY_API");
        assert_eq!(idents.ts_prop, "myApi");
        assert_eq!(idents.ts_type, "MyApi");
        assert_eq!(idents.ts_const, "MY_API");
    }

    #[test]
    fn remap_api_2() {
        let idents = remap_dns("api-2").unwrap();
        assert_eq!(idents.rust_field, "api_n2");
        assert_eq!(idents.rust_variant, "ApiN2");
        assert_eq!(idents.rust_const, "API_N2");
        assert_eq!(idents.ts_prop, "apiN2");
        assert_eq!(idents.ts_type, "ApiN2");
        assert_eq!(idents.ts_const, "API_N2");
    }

    #[test]
    fn remap_type_reserved() {
        let idents = remap_dns("type").unwrap();
        assert_eq!(idents.rust_field, "svc_type");
        assert_eq!(idents.rust_variant, "SvcType");
        assert_eq!(idents.rust_const, "SVC_TYPE");
        assert_eq!(idents.ts_prop, "svcType");
        assert_eq!(idents.ts_type, "SvcType");
        assert_eq!(idents.ts_const, "SVC_TYPE");
    }

    #[test]
    fn remap_rejects_self() {
        let err = remap_dns("self").unwrap_err();
        assert!(matches!(err, IdlError::ReservedWireName { .. }));
    }

    #[test]
    fn remap_rejects_crate_and_super() {
        assert!(matches!(
            remap_dns("crate").unwrap_err(),
            IdlError::ReservedWireName { .. }
        ));
        assert!(matches!(
            remap_dns("super").unwrap_err(),
            IdlError::ReservedWireName { .. }
        ));
    }
}
