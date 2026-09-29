//! A fixed-size native canvas with a bounded Cairo cache.
//!
//! GTK retains render nodes, but its Cairo renderer re-blurs CSS shadows and
//! downloads/converts textures whenever it paints them. Cache just those static
//! leaves, keeping all animated nodes and the widget/picking tree live.
//! Keep the native allocation fixed too: compositors can animate layer-surface
//! resizes by scaling the whole buffer, distorting our independent animations.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
};

use gtk::{cairo, gdk, glib, graphene, gsk, prelude::*, subclass::prelude::*};

const CACHE_BYTES: usize = 8 * 1024 * 1024;
const CACHE_ENTRIES: usize = 128;

#[derive(Hash, PartialEq, Eq)]
enum PaintKey {
    Shadow(glib::Bytes, i32),
    Texture(gdk::Texture, [u32; 4]),
}

struct Paint {
    node: gsk::RenderNode,
    bytes: usize,
    seen: u64,
}

#[derive(Default)]
struct PaintCache {
    leaves: HashMap<PaintKey, Paint>,
    bytes: usize,
    generation: u64,
}

impl PaintCache {
    fn render(&mut self, node: &gsk::RenderNode, scale: i32) -> gsk::RenderNode {
        self.generation = self.generation.wrapping_add(1);
        let result = self.rewrite(node, scale);
        // Closing a page releases its cached pixels on the very next snapshot.
        // Retaining keys also keeps texture identities alive until eviction.
        self.leaves.retain(|_, paint| paint.seen == self.generation);
        self.bytes = self.leaves.values().map(|paint| paint.bytes).sum();
        result
    }

    fn rewrite(&mut self, node: &gsk::RenderNode, scale: i32) -> gsk::RenderNode {
        if let Some(container) = node.downcast_ref::<gsk::ContainerNode>() {
            let children: Vec<_> = (0..container.n_children())
                .map(|i| self.rewrite(&container.child(i), scale))
                .collect();
            return gsk::ContainerNode::new(&children).upcast();
        }
        if let Some(transform) = node.downcast_ref::<gsk::TransformNode>() {
            return gsk::TransformNode::new(
                self.rewrite(&transform.child(), scale),
                Some(&transform.transform()),
            )
            .upcast();
        }
        if let Some(clip) = node.downcast_ref::<gsk::RoundedClipNode>() {
            return gsk::RoundedClipNode::new(self.rewrite(&clip.child(), scale), &clip.clip())
                .upcast();
        }
        if let Some(clip) = node.downcast_ref::<gsk::ClipNode>() {
            return gsk::ClipNode::new(self.rewrite(&clip.child(), scale), &clip.clip()).upcast();
        }
        if let Some(opacity) = node.downcast_ref::<gsk::OpacityNode>() {
            return gsk::OpacityNode::new(self.rewrite(&opacity.child(), scale), opacity.opacity())
                .upcast();
        }
        if let Some(fade) = node.downcast_ref::<gsk::CrossFadeNode>() {
            return gsk::CrossFadeNode::new(
                self.rewrite(&fade.start_child(), scale),
                self.rewrite(&fade.end_child(), scale),
                fade.progress(),
            )
            .upcast();
        }

        let bounds = node.bounds();
        let (key, width, height) = if let Some(texture) = node.downcast_ref::<gsk::TextureNode>() {
            let texture = texture.texture();
            let width = texture.width();
            let height = texture.height();
            (
                PaintKey::Texture(
                    texture,
                    [
                        bounds.x().to_bits(),
                        bounds.y().to_bits(),
                        bounds.width().to_bits(),
                        bounds.height().to_bits(),
                    ],
                ),
                width,
                height,
            )
        } else if node.is::<gsk::OutsetShadowNode>() || node.is::<gsk::InsetShadowNode>() {
            (
                PaintKey::Shadow(node.serialize(), scale),
                ((bounds.x() + bounds.width()).ceil() - bounds.x().floor()) as i32 * scale,
                ((bounds.y() + bounds.height()).ceil() - bounds.y().floor()) as i32 * scale,
            )
        } else {
            return node.clone();
        };
        if let Some(paint) = self.leaves.get_mut(&key) {
            paint.seen = self.generation;
            return paint.node.clone();
        }
        let Some(bytes) = pixel_bytes(width, height) else {
            return node.clone();
        };
        if self.leaves.len() >= CACHE_ENTRIES || bytes > CACHE_BYTES.saturating_sub(self.bytes) {
            return node.clone();
        }
        let Ok(surface) = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height) else {
            return node.clone();
        };
        let cached = gsk::CairoNode::new(&bounds);
        let cr = cached.draw_context();
        let result = match &key {
            PaintKey::Texture(texture, _) => {
                // Texture::download uses Cairo's native premultiplied ARGB32
                // format (BGRA bytes on little-endian). Convert once, rather
                // than waking GDK's entire conversion pool on every frame.
                let mut surface = surface;
                let stride = surface.stride() as usize;
                if let Ok(mut data) = surface.data() {
                    texture.download(&mut data, stride);
                } else {
                    return node.clone();
                }
                cr.translate(f64::from(bounds.x()), f64::from(bounds.y()));
                cr.scale(
                    f64::from(bounds.width()) / f64::from(width),
                    f64::from(bounds.height()) / f64::from(height),
                );
                cr.set_source_surface(&surface, 0.0, 0.0).and_then(|_| {
                    cr.source().set_extend(cairo::Extend::Pad);
                    cr.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
                    cr.fill()
                })
            }
            PaintKey::Shadow(_, _) => {
                surface.set_device_scale(f64::from(scale), f64::from(scale));
                let Ok(draw) = cairo::Context::new(&surface) else {
                    return node.clone();
                };
                let x = f64::from(bounds.x().floor());
                let y = f64::from(bounds.y().floor());
                draw.translate(-x, -y);
                node.draw(&draw);
                drop(draw);
                cr.set_source_surface(&surface, x, y)
                    .and_then(|_| cr.paint())
            }
        };
        if result.is_err() {
            return node.clone();
        }
        drop(cr);
        let cached = cached.upcast();
        self.bytes += bytes;
        self.leaves.insert(
            key,
            Paint {
                node: cached.clone(),
                bytes,
                seen: self.generation,
            },
        );
        cached
    }
}

fn pixel_bytes(width: i32, height: i32) -> Option<usize> {
    if width <= 0 || height <= 0 {
        return None;
    }
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Canvas {
        pub child: RefCell<Option<gtk::Widget>>,
        pub logical_size: Cell<(i32, i32)>,
        cache: RefCell<PaintCache>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Canvas {
        const NAME: &'static str = "MithshellCanvas";
        type Type = super::Canvas;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Canvas {
        fn dispose(&self) {
            if let Some(child) = self.child.borrow_mut().take() {
                child.unparent();
            }
            *self.cache.borrow_mut() = PaintCache::default();
        }
    }

    impl WidgetImpl for Canvas {
        fn measure(&self, orientation: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            let size = if orientation == gtk::Orientation::Horizontal {
                self.logical_size.get().0
            } else {
                self.logical_size.get().1
            };
            (size, size, -1, -1)
        }

        fn size_allocate(&self, _: i32, _: i32, baseline: i32) {
            if let Some(child) = self.child.borrow().as_ref() {
                let (width, height) = self.logical_size.get();
                child.allocate(width, height, baseline, None);
            }
        }

        fn unmap(&self) {
            self.parent_unmap();
            *self.cache.borrow_mut() = PaintCache::default();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let Some(child) = self.child.borrow().as_ref().cloned() else {
                return;
            };
            let capture = gtk::Snapshot::new();
            obj.snapshot_child(&child, &capture);
            let Some(node) = capture.to_node() else {
                *self.cache.borrow_mut() = PaintCache::default();
                return;
            };
            let cairo = obj
                .native()
                .and_then(|native| native.renderer())
                .is_some_and(|renderer| renderer.is::<gsk::CairoRenderer>());
            let node = if cairo {
                self.cache.borrow_mut().render(&node, obj.scale_factor())
            } else {
                node
            };
            snapshot.push_clip(&graphene::Rect::new(
                0.0,
                0.0,
                obj.width() as f32,
                obj.height() as f32,
            ));
            snapshot.append_node(&node);
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    pub struct Canvas(ObjectSubclass<imp::Canvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Canvas {
    pub(crate) fn new(child: &impl IsA<gtk::Widget>, width: i32, height: i32) -> Self {
        let canvas: Self = glib::Object::new();
        canvas.set_overflow(gtk::Overflow::Hidden);
        canvas.imp().logical_size.set((width, height));
        child.set_parent(&canvas);
        canvas.imp().child.replace(Some(child.clone().upcast()));
        canvas
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels(node: &gsk::RenderNode, scale: i32) -> Vec<u8> {
        let mut surface =
            cairo::ImageSurface::create(cairo::Format::ARgb32, 160 * scale, 100 * scale).unwrap();
        surface.set_device_scale(f64::from(scale), f64::from(scale));
        let cr = cairo::Context::new(&surface).unwrap();
        node.draw(&cr);
        drop(cr);
        surface.data().unwrap().to_vec()
    }

    #[test]
    #[ignore = "requires the project-local Broadway runner"]
    fn cairo_cache_preserves_pixels_reuses_leaves_and_evicts_old_content() {
        gtk::init().unwrap();
        let texture = gdk::MemoryTexture::new(
            2,
            2,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(vec![
                255u8, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 64, 255, 255, 0, 0,
            ]),
            8,
        );
        let texture: gsk::RenderNode =
            gsk::TextureNode::new(&texture, &graphene::Rect::new(30.0, 24.0, 40.0, 40.0)).upcast();
        let outline =
            gsk::RoundedRect::from_rect(graphene::Rect::new(20.0, 20.0, 100.0, 50.0), 12.0);
        let shadow: gsk::RenderNode = gsk::OutsetShadowNode::new(
            &outline,
            &gdk::RGBA::new(0.2, 0.1, 0.3, 0.4),
            0.0,
            3.0,
            1.0,
            10.0,
        )
        .upcast();
        for scale in [1, 2] {
            let mut cache = PaintCache::default();
            let root = gsk::ContainerNode::new(&[shadow.clone(), texture.clone()]).upcast();
            let cached = cache.render(&root, scale);
            assert_eq!(cache.leaves.len(), 2);
            let original = pixels(&root, scale);
            let raster = pixels(&cached, scale);
            let error = original
                .iter()
                .zip(&raster)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(
                error <= 1,
                "cached rendering changed pixels at scale {scale}: {error}"
            );
            let first = cache.render(&texture, scale);
            let second = cache.render(&texture, scale);
            assert_eq!(
                first.as_ptr(),
                second.as_ptr(),
                "texture conversion must be reused"
            );
            assert_eq!(cache.leaves.len(), 1, "unused shadow must be evicted");
            let changed =
                gsk::OutsetShadowNode::new(&outline, &gdk::RGBA::RED, 0.0, 3.0, 1.0, 10.0).upcast();
            let new_shadow = cache.render(&changed, scale);
            assert_ne!(
                pixels(&new_shadow, scale),
                pixels(&shadow, scale),
                "theme change must invalidate the shadow"
            );
            cache.render(
                &gsk::ColorNode::new(&gdk::RGBA::BLACK, outline.bounds()).upcast(),
                scale,
            );
            assert_eq!(cache.bytes, 0);
            assert!(cache.leaves.is_empty());
        }
    }
}
