//! CreateGfxControl case tables: N3 0x100ce3be–0x100d145c.
//! A missing case is a native null, not an unimplemented authored class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Creation {
    Unlocated,
    Vector,
    #[default]
    Matrix,
    RConnector,
    HitLocation,
    Dynel,
    Tracer,
    VectorVector,
    VectorMatrix,
    VectorConnector,
    VectorDynel,
    DynelVector,
    DynelMatrix,
    DynelConnector,
    DynelDynel,
}

impl Creation {
    pub fn constructs(self, class: i32) -> bool {
        use Creation::*;
        match class {
            1000 | 1014 | 1015 | 1016 | 1020 | 3037 => matches!(self, Unlocated),
            1001 | 1017 | 2011 | 2012 | 2014 | 3001 | 3003 | 3008 | 3034 => matches!(self, Dynel),
            1002 | 1003 | 1004 | 1006 | 1007 | 1008 | 1009 | 1012 | 1023 | 1029 |
            3004 | 3005 | 3006 | 3014 | 3022 | 3023 | 3024 | 3025 | 3027 | 3029 |
            3030 | 3031 | 3032 | 3033 | 3035 | 3036 | 3038 | 3039 => matches!(self, Vector | Matrix | RConnector | Dynel),
            1005 | 1018 | 3015 | 3019 | 3020 | 3028 | 4000 => matches!(self, Vector | Matrix | RConnector | HitLocation | Dynel),
            1010 => [DynelVector, DynelMatrix, DynelConnector, DynelDynel].contains(&self),
            1011 => [VectorVector, VectorMatrix, VectorConnector, VectorDynel].contains(&self),
            1013 | 1019 | 1021 | 1022 | 1024 | 1025 | 1026 | 1027 | 3026 => [HitLocation, Tracer].contains(&self),
            1028 | 2000 => matches!(self, Vector),
            2001 | 2008 | 3002 | 3013 | 3018 => matches!(self, Vector | Dynel),
            2002 | 2010 | 2013 => matches!(self, HitLocation),
            2004 | 3011 => matches!(self, Vector | HitLocation | Dynel),
            2005 => matches!(self, HitLocation | Dynel),
            2006 => matches!(self, RConnector | Dynel),
            2007 => matches!(self, Vector | RConnector | HitLocation | Dynel),
            2009 | 3000 | 3009 | 3010 | 3012 | 3016 => matches!(self, Vector | Matrix | Dynel),
            2015 => matches!(self, Vector | RConnector | Dynel),
            3007 => matches!(self, VectorVector | Dynel),
            3017 => matches!(self, Matrix | RConnector | HitLocation | Dynel),
            3021 | 5000 => matches!(self, RConnector),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Creation::*;
    #[test]
    fn native_overload_cases_and_nulls() {
        let overloads = [Unlocated, Vector, Matrix, RConnector, HitLocation, Dynel, Tracer,
            VectorVector, VectorMatrix, VectorConnector, VectorDynel, DynelVector,
            DynelMatrix, DynelConnector, DynelDynel];
        for class in [0, 2003] {
            assert!(overloads.iter().all(|overload| !overload.constructs(class)));
        }
        for (class, cases) in [
            (1010, &[DynelVector, DynelMatrix, DynelConnector, DynelDynel][..]),
            (1011, &[VectorVector, VectorMatrix, VectorConnector, VectorDynel][..]),
            (3017, &[Matrix, RConnector, HitLocation, Dynel][..]),
            (2002, &[HitLocation][..]), (2013, &[HitLocation][..]),
            (5000, &[RConnector][..]), (1020, &[Unlocated][..]),
            (1027, &[HitLocation, Tracer][..]),
        ] {
            for overload in overloads { assert_eq!(overload.constructs(class), cases.contains(&overload), "{class}: {overload:?}"); }
        }
    }
}
