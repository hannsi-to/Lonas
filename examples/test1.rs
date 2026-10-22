use Lonas::lonas::{Camera, DrawManager2D, DrawManager3D, GpuContext, Instance, MemoryUpdateType, Object2D, Shader};

fn main() {

}

pub struct TestApplication {
    objects: Objects,
    gpu: GpuContext,
    draw_manager_2d: DrawManager2D,
    draw_manager_3d: DrawManager3D,
    x: f32,
    reverse: bool,
}

pub struct Objects {
    pub object1: Option<Object2D>,
}

impl TestApplication {
    pub fn init() -> Self {
        let mut gpu = GpuContext::new();
        let draw_manager_2d = DrawManager2D::new(&mut gpu, Camera::default());
        let draw_manager_3d = DrawManager3D::new(&mut gpu, Camera::default());

        // 頂点を毎フレーム動かすため Dynamic
        let mut object1 = Object2D::new();
        object1.memory_update_type(MemoryUpdateType::Dynamic { ring_count: 3 });

        Self {
            objects: Objects {
                object1: Some(object1)
            },
            gpu,
            draw_manager_2d,
            draw_manager_3d,
            x: 100.0,
            reverse: false,
        }
    }

    pub fn draw_loop(&mut self) {
        if self.x > 100.0 {
            self.reverse = true;
        }

        if self.x < 0.0 {
            self.reverse = false;
        }

        if self.reverse {
            self.x -= 1.0;
        } else {
            self.x += 1.0;
        }

        self.gpu.begin_frame();

        // 毎フレーム必ず呼ばれる関数群
        self.objects.object1.as_mut().unwrap().begin();
        self.objects.object1.as_mut().unwrap().vertex(self.x,100.0,0.0);
        self.objects.object1.as_mut().unwrap().vertex(self.x,100.0,0.0);
        self.objects.object1.as_mut().unwrap().vertex(self.x,300.0,0.0);
        self.objects.object1.as_mut().unwrap().end(&mut self.draw_manager_2d);

        {
            // 高頻度で呼ばれるわけではないが、描画中でも呼ばれる関数群
            self.objects.object1.as_mut().unwrap().shader(Shader::default());
            self.objects.object1.as_mut().unwrap().add_instance(Instance::default());
        }

        // 渡した順に描画する（3D → 2D）
        self.gpu.end_frame(&mut [&mut self.draw_manager_3d, &mut self.draw_manager_2d]);
    }

    pub fn finish(&mut self) {

    }
}
