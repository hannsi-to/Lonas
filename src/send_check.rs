// 公開する型が別のスレッドへ渡せること（§11 スレッド）
fn assert_send<T: Send>() {}

#[test]
fn public_types_are_send() {
    assert_send::<crate::object::Object2D>();
    assert_send::<crate::object::Object3D>();
    assert_send::<crate::draw::DrawManager2D>();
    assert_send::<crate::draw::DrawManager3D>();
    assert_send::<crate::resource::ResourceManager>();
    assert_send::<crate::gpu::GpuContext>();
}
