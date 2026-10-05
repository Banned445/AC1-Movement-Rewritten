//! Puts Altaïr's real model (loaded from the user's install) on the player, replacing the capsule,
//! as a skinned mesh driven by his 90-bone skeleton.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::assets::altair::load_altair;
use crate::assets::ac_formats::CHARACTER_VISUAL_FIXES;
use crate::assets::game_dir;
use crate::player::Player;

/// Marks the stand-in capsule meshes so they can be hidden once the real model is in.
#[derive(Component)]
pub struct PlaceholderBody;

#[derive(Resource, Default)]
pub struct ModelStatus(pub String);

/// The character rig: joint entities (skeleton order) and their rest local transforms.
#[derive(Component)]
pub struct Rig {
    /// Skeleton root entity (its transform maps skeleton/animation space to player space).
    pub root: Entity,
    pub joints: Vec<Entity>,
    pub rest: Vec<Transform>,
    pub bone_ids: Vec<u32>,
    pub parents: Vec<Option<usize>>,
}

pub struct ModelPlugin;

impl Plugin for ModelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ModelStatus>().add_systems(PostStartup, attach_altair);
    }
}

/// Skeleton space (Z-up, X0-rotated) → player-local Bevy space (Y-up, facing -Z):
/// skel (x, y, z) → model (y, -x, z) → Bevy (-y, z, -x), feet lifted to y = 0.
fn skeleton_root(min_z: f32) -> Transform {
    let m = Mat3::from_cols(Vec3::new(0.0, 0.0, -1.0), Vec3::new(-1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
    Transform { translation: Vec3::new(0.0, -min_z, 0.0), rotation: Quat::from_mat3(&m), scale: Vec3::ONE }
}

#[allow(clippy::too_many_arguments)]
fn attach_altair(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut status: ResMut<ModelStatus>,
    player: Query<Entity, With<Player>>,
    mut placeholders: Query<&mut Visibility, With<PlaceholderBody>>,
) {
    let Ok(player) = player.single() else { return };
    if std::env::var_os("AC_NO_MODEL").is_some() {
        status.0 = "model: disabled (AC_NO_MODEL)".into();
        return;
    }
    let model = match load_altair(&game_dir()) {
        Ok(m) => m,
        Err(e) => {
            status.0 = format!("model: not loaded ({e}); run once with --game-dir \"<your Assassin's Creed folder>\"");
            warn!("{}", status.0);
            return;
        }
    };

    // ---------------------------------------------------------------- skeleton → joint entities
    let root_t = skeleton_root(model.min_z);
    let root = commands.spawn((root_t, Visibility::default(), Name::new("skeleton_root"))).id();
    commands.entity(player).add_child(root);
    let n = model.skeleton.len();
    let mut joints = Vec::with_capacity(n);
    let mut rest = Vec::with_capacity(n);
    let mut global: Vec<Mat4> = Vec::with_capacity(n);
    for b in &model.skeleton {
        let t = Transform {
            translation: Vec3::from_array(b.local_pos),
            rotation: Quat::from_array(b.local_rot).normalize(),
            scale: Vec3::ONE,
        };
        let parent_global = b.parent.map(|p| global[p]).unwrap_or_else(|| root_t.to_matrix());
        global.push(parent_global * t.to_matrix());
        let e = commands.spawn((t, Visibility::default())).id();
        let parent_entity = b.parent.map(|p| joints[p]).unwrap_or(root);
        commands.entity(parent_entity).add_child(e);
        joints.push(e);
        rest.push(t);
    }
    let inv: Vec<Mat4> = global.iter().map(|g| g.inverse()).collect();
    let skinned = !joints.is_empty();
    let inv_handle = bindposes.add(SkinnedMeshInverseBindposes::from(inv));

    // ---------------------------------------------------------------- textures
    let mut tex_handles = std::collections::HashMap::new();
    for (id, t) in model.textures.iter().chain(model.normal_textures.iter()) {
        let mut img = Image::new_uninit(
            Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
            TextureDimension::D2,
            if model.normal_textures.contains_key(id) { TextureFormat::Rgba8Unorm } else { TextureFormat::Rgba8UnormSrgb },
            RenderAssetUsages::RENDER_WORLD,
        );
        img.texture_descriptor.mip_level_count = t.mips.len() as u32;
        img.data = Some(t.mips.concat());
        if CHARACTER_VISUAL_FIXES {
            // The outfit UVs tile beyond [0, 1]; clamp sampling smears the atlas border (RE/09 §6).
            img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                ..ImageSamplerDescriptor::linear()
            });
        }
        tex_handles.insert(*id, images.add(img));
    }
    let untextured = materials.add(StandardMaterial {
        base_color: Color::srgb(0.85, 0.85, 0.82),
        perceptual_roughness: 0.9,
        double_sided: true,
        cull_mode: None,
        ..default()
    });

    // ---------------------------------------------------------------- meshes
    let mut tris = 0;
    for part in &model.parts {
        for ((idx, tex), normal) in part.sections.iter().zip(&part.normal_maps) {
            tris += idx.len() / 3;
            let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, part.positions.clone());
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, part.normals.clone());
            mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, part.tangents.clone());
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, part.uvs.clone());
            if skinned {
                mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(part.joints.clone()));
                mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, part.weights.clone());
            }
            mesh.insert_indices(Indices::U32(idx.clone()));
            let mat = match tex.and_then(|t| tex_handles.get(&t)) {
                // Cloth (robe, flaps, hood) is single-layer geometry the game draws two-sided; with back-face
                // culling the inner side disappears and the body shows through. BC1 textures carry 1-bit alpha for
                // frayed edges and feathers: alpha-tested.
                Some(h) => materials.add(StandardMaterial {
                    base_color_texture: Some(h.clone()),
                    normal_map_texture: if CHARACTER_VISUAL_FIXES { normal.and_then(|id| tex_handles.get(&id)).cloned() } else { None },
                    // PORT: DirectX normal-map convention; the game's shader lighting is not yet reproduced (RE/09 §6).
                    flip_normal_map_y: true,
                    perceptual_roughness: 0.85,
                    double_sided: true,
                    cull_mode: None,
                    alpha_mode: AlphaMode::Mask(0.5),
                    ..default()
                }),
                None => untextured.clone(),
            };
            let mut ec = commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(mat), Transform::default()));
            if skinned {
                ec.insert(SkinnedMesh { inverse_bindposes: inv_handle.clone(), joints: joints.clone() });
            }
            let child = ec.id();
            commands.entity(player).add_child(child);
        }
    }
    for mut v in &mut placeholders {
        *v = Visibility::Hidden;
    }
    commands.entity(player).insert((crate::anim::AnimPlayer::default(), Rig {
        root,
        joints,
        rest,
        bone_ids: model.skeleton.iter().map(|b| b.bone_id).collect(),
        parents: model.skeleton.iter().map(|b| b.parent).collect(),
    }));
    status.0 = format!(
        "model: Altair from your install ({} parts, {} tris, {} textures, {} bones)",
        model.parts.len(),
        tris,
        model.textures.len(),
        n
    );
    info!("{} — {}", status.0, model.source);
}
