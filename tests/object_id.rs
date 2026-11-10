// ObjectId を crate の外（Aevon など）から HashMap のキー・比べる値として使えること（§2 Object* の識別子）

use std::collections::{BTreeMap, HashMap};

use lonas::lonas::{Instance2D, InstanceId, Object2D, Object3D, ObjectId};

#[allow(dead_code)]
struct Owners {
    by_object: HashMap<ObjectId, &'static str>,
    ordered: BTreeMap<ObjectId, u64>,
}

#[allow(dead_code)]
fn record(owners: &mut Owners, object_2d: &Object2D, object_3d: &Object3D, instance: InstanceId<Instance2D>) -> bool {
    owners.by_object.insert(object_2d.id(), "2d");
    owners.by_object.insert(object_3d.id(), "3d");
    owners.ordered.insert(object_2d.id(), object_2d.id().get());
    let _label: String = object_2d.id().to_string();

    instance.object() == object_2d.id()
}

#[test]
fn object_id_is_usable_from_outside_the_crate() {
    // GPU なしで確かめられるのは型と関数の形まで。値の振る舞いは crate の中のテストで見る
    let _ = record as fn(&mut Owners, &Object2D, &Object3D, InstanceId<Instance2D>) -> bool;
}
