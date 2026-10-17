//! Surface publique du crate `vanyline-cfgstore` vue d'un crate consommateur
//! (spec `cfgstore/src/lib.sdd`, Scenarios de surface). Tests d'intégration :
//! seuls les chemins publics sont employés.

use std::any::type_name;

use vanyline_cfgstore::CfgStoreError;

#[test]
fn surface_racine_cfgstore_error_canonique() {
    let err = CfgStoreError::Config("boom".to_string());
    assert!(format!("{err}").contains("VNL-CFG-001"));
}

#[test]
fn surface_error_double_chemin_meme_item() {
    let identite: fn(vanyline_cfgstore::CfgStoreError) -> vanyline_cfgstore::error::CfgStoreError =
        std::convert::identity;
    let err = identite(CfgStoreError::ReadOnly);
    assert!(matches!(err, CfgStoreError::ReadOnly));
}

#[test]
fn surface_modules_publics_adressables() {
    use vanyline_cfgstore::domain::Agent;
    use vanyline_cfgstore::error::CfgStoreError as ErreurLongue;
    use vanyline_cfgstore::fs_store::FsConfigStore;
    use vanyline_cfgstore::layers::Layers;
    use vanyline_cfgstore::store::{ConfigStore, Layer};

    let noms = [
        type_name::<Agent>(),
        type_name::<ErreurLongue>(),
        type_name::<FsConfigStore>(),
        type_name::<Layers>(),
        type_name::<Layer>(),
        type_name::<dyn ConfigStore>(),
    ];
    assert!(
        noms.iter().all(|n| n.contains("vanyline_cfgstore")),
        "{noms:?}"
    );
}
