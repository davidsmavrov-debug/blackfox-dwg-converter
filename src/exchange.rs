use crate::{output_format, read, read_for_import, Format, Result};
use acadrust::entities::acis::{
    SatCoedge, SatDocument, SatEdge, SatIntCurve, SatLoop, SatPoint, SatVertex, Sense,
};
use acadrust::entities::{
    Block, BlockEnd, Dimension, DimensionAligned, EntityType, Face3D, Insert, Line, Mesh, MeshFace,
    Point, Text,
};
use acadrust::entities::{DimensionLinear, Leader, MText, Polyline, XLine};
use acadrust::tables::{BlockRecord, Layer};
use acadrust::types::{Color, Handle, Matrix3, Matrix4, Vector3};
use acadrust::{CadDocument, DwgWriter, DxfVersion, DxfWriter};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

type Position = [f64; 3];

#[derive(Deserialize, Serialize)]
struct Drawing {
    schema: u32,
    units: i16,
    #[serde(default)]
    layers: BTreeMap<String, LayerProperties>,
    entities: Vec<Record>,
    #[serde(default)]
    blocks: BTreeMap<String, Vec<Record>>,
    #[serde(default)]
    omitted: BTreeMap<String, usize>,
}

#[derive(Deserialize, Serialize)]
struct LayerProperties {
    visible: bool,
    locked: bool,
    rgb: Option<(u8, u8, u8)>,
}

#[derive(Deserialize, Serialize)]
struct Record {
    layer: String,
    #[serde(flatten)]
    geometry: Geometry,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Geometry {
    Line {
        start: Position,
        end: Position,
    },
    Point {
        position: Position,
    },
    GuideLine {
        position: Position,
        direction: Position,
    },
    Polyline {
        vertices: Vec<Position>,
        closed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approximation: Option<String>,
    },
    Face {
        corners: Vec<Position>,
    },
    Mesh {
        vertices: Vec<Position>,
        faces: Vec<Vec<usize>>,
    },
    BrepMesh {
        vertices: Vec<Position>,
        faces: Vec<Vec<usize>>,
        holes: Vec<Vec<Vec<usize>>>,
    },
    Instance {
        block: String,
        transform: [[f64; 4]; 4],
    },
    AlignedDimension {
        start: Position,
        end: Position,
        dimension_line: Position,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        axis: Option<char>,
        text: String,
    },
    Leader {
        anchor: Position,
        label_position: Position,
        text: String,
        height: f64,
    },
    Text {
        position: Position,
        text: String,
        height: f64,
        rotation: f64,
    },
}

fn position(value: Vector3) -> Position {
    [value.x, value.y, value.z]
}

fn vector([x, y, z]: Position) -> Vector3 {
    Vector3::new(x, y, z)
}

fn spline_basis(degree: usize, knots: &[f64], count: usize, t: f64) -> Option<Vec<f64>> {
    if degree >= count || knots.len() != count + degree + 1 {
        return None;
    }
    let span = (degree..count)
        .find(|&index| t < knots[index + 1])
        .unwrap_or(count - 1);
    let mut basis = vec![0.0; degree + 1];
    basis[0] = 1.0;
    for level in 1..=degree {
        let mut saved = 0.0;
        for index in 0..level {
            let left = t - knots[span + 1 - level + index];
            let right = knots[span + 1 + index] - t;
            let width = left + right;
            let term = if width > 0.0 {
                basis[index] / width
            } else {
                0.0
            };
            basis[index] = saved + right * term;
            saved = left * term;
        }
        basis[level] = saved;
    }
    let mut weights = vec![0.0; count];
    weights[span - degree..=span].copy_from_slice(&basis);
    Some(weights)
}

fn spline_patch(
    document: &SatDocument,
    face: &acadrust::entities::acis::SatFace<'_>,
) -> Option<(Vec<Vector3>, Vec<Vec<usize>>)> {
    use acadrust::entities::acis::SatSplineSurface;

    let spline = SatSplineSurface::from_record(document.resolve(face.surface())?)?;
    let net = spline.bspline(document)?;
    let boundary = SatLoop::from_record(document.resolve(face.first_loop())?)?;
    if !boundary.next_loop().is_null() {
        return None;
    }
    let mut boundary_vertices = Vec::new();
    let first = boundary.first_coedge();
    let mut current = first;
    let mut visited = std::collections::BTreeSet::new();
    loop {
        if !visited.insert(current.0) {
            return None;
        }
        let coedge = SatCoedge::from_record(document.resolve(current)?)?;
        let edge = SatEdge::from_record(document.resolve(coedge.edge())?)?;
        for pointer in [edge.start_vertex(), edge.end_vertex()] {
            let vertex = SatVertex::from_record(document.resolve(pointer)?)?;
            let (x, y, z) = SatPoint::from_record(document.resolve(vertex.point())?)?.position();
            boundary_vertices.push(Vector3::new(x, y, z));
        }
        current = coedge.next();
        if current.0 == first.0 {
            break;
        }
    }

    const STEPS: usize = 32;
    const SIDE: usize = STEPS + 1;
    let samples = |degree: usize, knots: &[f64], count: usize| -> Option<Vec<Vec<f64>>> {
        let first = *knots.get(degree)?;
        let last = *knots.get(count)?;
        if !first.is_finite() || !last.is_finite() || last <= first {
            return None;
        }
        (0..SIDE)
            .map(|i| {
                spline_basis(
                    degree,
                    knots,
                    count,
                    first + (last - first) * i as f64 / STEPS as f64,
                )
            })
            .collect()
    };
    let us = samples(net.degree_u, &net.u_knots, net.control_count_u)?;
    let vs = samples(net.degree_v, &net.v_knots, net.control_count_v)?;
    if net.control_count_u.checked_mul(net.control_count_v)? != net.control_points.len() {
        return None;
    }
    let mut points = Vec::with_capacity(SIDE * SIDE);
    for v in &vs {
        for u in &us {
            let mut coordinates = [0.0; 4];
            for (row, v_weight) in v.iter().enumerate().filter(|(_, w)| **w != 0.0) {
                for (col, u_weight) in u.iter().enumerate().filter(|(_, w)| **w != 0.0) {
                    let weight = v_weight * u_weight;
                    for (value, control) in coordinates
                        .iter_mut()
                        .zip(net.control_points[row * net.control_count_u + col])
                    {
                        *value += weight * control;
                    }
                }
            }
            let [x, y, z, w] = coordinates;
            if w.abs() < 1e-12 || !coordinates.iter().all(|value| value.is_finite()) {
                return None;
            }
            points.push(Vector3::new(x / w, y / w, z / w));
        }
    }
    let corners = [0, STEPS, STEPS * SIDE, SIDE * SIDE - 1];
    let origin = boundary_vertices.first()?;
    let size = points
        .iter()
        .map(|point| point.distance(origin))
        .fold(1.0, f64::max);
    let tolerance = size * 1e-5;
    // Require the boundary to reach all four corners, including collapsed poles.
    if corners.iter().any(|&i| {
        !boundary_vertices
            .iter()
            .any(|vertex| vertex.distance(&points[i]) <= tolerance)
    }) {
        return None;
    }
    let mut faces = Vec::with_capacity(STEPS * STEPS * 2);
    for row in 0..STEPS {
        for col in 0..STEPS {
            let a = row * SIDE + col;
            for triangle in [[a, a + 1, a + SIDE + 1], [a, a + SIDE + 1, a + SIDE]] {
                let [p, q, r] = triangle.map(|index| points[index]);
                if (q - p).cross(&(r - p)).length() > size * size * 1e-12 {
                    faces.push(triangle.to_vec());
                }
            }
        }
    }
    (!faces.is_empty()).then_some((points, faces))
}

// Build editable faces from supported ACIS planes and spline patches.
// Omit the body if a face cannot be reconstructed.
fn planar_solid(document: &SatDocument) -> Option<Geometry> {
    let bodies = document.bodies();
    if bodies.len() != 1 {
        return None;
    }
    let (matrix, translation, scale) = document.placement();
    let placement = Matrix3::from_rows(matrix[0], matrix[1], matrix[2]).transpose();
    let mirrored = placement.determinant() * scale < 0.0;
    if !scale.is_finite() || scale == 0.0 || placement.determinant().abs() < 1e-12 {
        return None;
    }
    let mut vertices = Vec::new();
    let mut vertex_map = BTreeMap::new();
    let mut faces = Vec::new();
    let mut holes = Vec::new();
    for face in document.faces() {
        if document.resolve(face.surface())?.entity_type != "plane-surface" {
            let (points, patch) = spline_patch(document, &face)?;
            let start = vertices.len();
            vertices.extend(points.into_iter().map(|point| {
                position(placement.transform_point(point) * scale + vector(translation))
            }));
            for mut ring in patch {
                for index in &mut ring {
                    *index += start;
                }
                if matches!(face.sense(), Sense::Reversed) != mirrored {
                    ring.reverse();
                }
                faces.push(ring);
                holes.push(Vec::new());
            }
            continue;
        }
        let mut boundary_pointer = face.first_loop();
        let mut visited_loops = std::collections::BTreeSet::new();
        let mut rings = Vec::new();
        while !boundary_pointer.is_null() {
            if !visited_loops.insert(boundary_pointer.0) {
                return None;
            }
            let boundary = SatLoop::from_record(document.resolve(boundary_pointer)?)?;
            let first = boundary.first_coedge();
            let mut current = first;
            let mut ring = Vec::new();
            let mut visited = std::collections::BTreeSet::new();
            let mut previous_end = None;
            let mut first_start = None;
            loop {
                if !visited.insert(current.0) {
                    return None;
                }
                let coedge = SatCoedge::from_record(document.resolve(current)?)?;
                let edge = SatEdge::from_record(document.resolve(coedge.edge())?)?;
                let curve = document.resolve(edge.curve())?;
                let (start, end) = match coedge.sense() {
                    Sense::Forward => (edge.start_vertex(), edge.end_vertex()),
                    Sense::Reversed => (edge.end_vertex(), edge.start_vertex()),
                };
                if previous_end.is_some_and(|previous| previous != start.0) {
                    return None;
                }
                first_start.get_or_insert(start.0);
                previous_end = Some(end.0);
                let index = if let Some(index) = vertex_map.get(&start.0) {
                    *index
                } else {
                    let vertex = SatVertex::from_record(document.resolve(start)?)?;
                    let point = SatPoint::from_record(document.resolve(vertex.point())?)?;
                    let (x, y, z) = point.position();
                    let index = vertices.len();
                    let world = placement.transform_point(Vector3::new(x, y, z)) * scale
                        + vector(translation);
                    if !position(world).iter().all(|value| value.is_finite()) {
                        return None;
                    }
                    vertices.push(position(world));
                    vertex_map.insert(start.0, index);
                    index
                };
                ring.push(index);
                if curve.entity_type != "straight-curve" {
                    let spline = SatIntCurve::from_record(curve)?;
                    let local_vertex = |pointer| {
                        let vertex = SatVertex::from_record(document.resolve(pointer)?)?;
                        let (x, y, z) =
                            SatPoint::from_record(document.resolve(vertex.point())?)?.position();
                        Some(Vector3::new(x, y, z))
                    };
                    let a = local_vertex(start)?;
                    let b = local_vertex(end)?;
                    let segment = b - a;
                    let length_squared = segment.length_squared();
                    let is_straight = length_squared > 1e-18
                        && spline.bspline_in(document).is_some_and(|(_, _, controls)| {
                            !controls.is_empty()
                                && controls.iter().all(|&[x, y, z, weight]| {
                                    if weight <= 0.0 || !weight.is_finite() {
                                        return false;
                                    }
                                    let delta =
                                        Vector3::new(x / weight, y / weight, z / weight) - a;
                                    let along = delta.dot(&segment) / length_squared;
                                    (-1e-8..=1.0 + 1e-8).contains(&along)
                                        && (delta - segment * along).length()
                                            <= 1e-8 * segment.length().max(1.0)
                                })
                        });
                    if is_straight {
                        current = coedge.next();
                        if current.0 == first.0 {
                            break;
                        }
                        continue;
                    }
                    let (lo, hi) = if spline.sense() == Sense::Reversed {
                        (-edge.end_param(), -edge.start_param())
                    } else {
                        (edge.start_param(), edge.end_param())
                    };
                    let sample_point = |&(x, y, z): &(f64, f64, f64)| Vector3::new(x, y, z);
                    let tolerance = 1e-5 * a.distance(&b).max(1.0);
                    let matches_edge = |samples: &mut Vec<(f64, f64, f64)>| {
                        let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
                            return false;
                        };
                        let (first, last) = (sample_point(first), sample_point(last));
                        if first.distance(&a) + last.distance(&b)
                            > first.distance(&b) + last.distance(&a)
                        {
                            samples.reverse();
                        }
                        sample_point(&samples[0]).distance(&a) <= tolerance
                            && sample_point(&samples[samples.len() - 1]).distance(&b) <= tolerance
                    };
                    let mut samples = spline.sample_range(lo, hi, 32);
                    if !matches_edge(&mut samples) {
                        samples = spline.sample(32);
                        if !matches_edge(&mut samples) {
                            return None;
                        }
                    }
                    for sample in &samples[1..samples.len() - 1] {
                        let p = sample_point(sample);
                        let world = placement.transform_point(p) * scale + vector(translation);
                        if !position(world).iter().all(|value| value.is_finite()) {
                            return None;
                        }
                        ring.push(vertices.len());
                        vertices.push(position(world));
                    }
                }
                current = coedge.next();
                if current.0 == first.0 {
                    break;
                }
            }
            if ring.len() < 3 || first_start != previous_end {
                return None;
            }
            if matches!(face.sense(), Sense::Reversed) != mirrored {
                ring.reverse();
            }
            rings.push(ring);
            boundary_pointer = boundary.next_loop();
        }
        let mut rings = rings.into_iter();
        faces.push(rings.next()?);
        holes.push(rings.collect());
    }
    if faces.is_empty() {
        return None;
    }
    Some(Geometry::BrepMesh {
        vertices,
        faces,
        holes,
    })
}

pub(super) fn decode(input: &Path, output: &Path) -> Result<()> {
    reject_overwrite(input, output)?;
    let (_, doc) = read_for_import(input)?;
    let mut drawing = Drawing {
        schema: 1,
        units: doc.header.insertion_units,
        layers: doc
            .layers
            .iter()
            .map(|layer| {
                (
                    layer.name.clone(),
                    LayerProperties {
                        visible: !layer.flags.off && !layer.flags.frozen,
                        locked: layer.flags.locked,
                        rgb: if layer.color == Color::Index(7) {
                            None
                        } else {
                            layer.color.rgb()
                        },
                    },
                )
            })
            .collect(),
        entities: Vec::new(),
        blocks: BTreeMap::new(),
        omitted: BTreeMap::new(),
    };
    let mut pending = std::collections::BTreeSet::new();
    drawing.entities = decode_entities(&doc, doc.model_space_entities(), &mut drawing.omitted);
    referenced_blocks(&drawing.entities, &mut pending);
    while let Some(name) = pending.pop_first() {
        if drawing.blocks.contains_key(&name) {
            continue;
        }
        let records = decode_entities(&doc, doc.entities_in_block(&name), &mut drawing.omitted);
        referenced_blocks(&records, &mut pending);
        drawing.blocks.insert(name, records);
    }
    let mut staged = temporary_output(output)?;
    serde_json::to_writer(staged.as_file_mut(), &drawing)?;
    staged.as_file().sync_all()?;
    staged.persist(output)?;
    Ok(())
}

fn referenced_blocks(records: &[Record], pending: &mut std::collections::BTreeSet<String>) {
    for record in records {
        if let Geometry::Instance { block, .. } = &record.geometry {
            pending.insert(block.clone());
        }
    }
}

fn sample_curve(
    start: f64,
    sweep: f64,
    closed: bool,
    sample: impl Fn(f64) -> Vector3,
) -> Vec<Position> {
    let count = ((sweep.abs() / (std::f64::consts::PI / 36.0)).ceil() as usize).clamp(1, 360);
    (0..count + usize::from(!closed))
        .map(|i| position(sample(start + sweep * i as f64 / count as f64)))
        .collect()
}

fn lwpolyline(poly: &acadrust::entities::LwPolyline) -> Geometry {
    let closed = poly.is_closed;
    let ocs = Matrix3::arbitrary_axis(poly.normal);
    let in_world = |x, y| ocs.transform_point(Vector3::new(x, y, poly.elevation));
    let mut vertices = Vec::new();
    let mut curved = false;
    for (i, start) in poly.vertices.iter().enumerate() {
        let a = start.location;
        vertices.push(position(in_world(a.x, a.y)));
        let next = poly
            .vertices
            .get(i + 1)
            .or_else(|| closed.then(|| &poly.vertices[0]));
        if let Some(end) = next {
            let b = end.location;
            let bulge = start.bulge;
            if bulge.abs() > 1e-12 && a != b {
                curved = true;
                let dx = b.x - a.x;
                let dy = b.y - a.y;
                let offset = (1.0 - bulge * bulge) / (4.0 * bulge);
                let x = (a.x + b.x) / 2.0 - dy * offset;
                let y = (a.y + b.y) / 2.0 + dx * offset;
                let radius = (a.x - x).hypot(a.y - y);
                let angle = (a.y - y).atan2(a.x - x);
                vertices.extend(
                    sample_curve(angle, 4.0 * bulge.atan(), true, |theta| {
                        in_world(x + radius * theta.cos(), y + radius * theta.sin())
                    })
                    .into_iter()
                    .skip(1),
                );
            }
        }
    }
    Geometry::Polyline {
        vertices,
        closed,
        approximation: curved.then(|| "LWPOLYLINE bulges".into()),
    }
}

fn dimension_geometry(dimension: &Dimension) -> Option<Geometry> {
    let (start, end, location, axis) =
        match dimension {
            Dimension::Aligned(dim) => (
                dim.first_point,
                dim.second_point,
                dim.definition_point,
                None,
            ),
            Dimension::Linear(dim) => {
                let direction = Matrix3::arbitrary_axis(dim.base.normal)
                    .transform_point(Vector3::new(dim.rotation.cos(), dim.rotation.sin(), 0.0));
                let axis = [('x', direction.x), ('y', direction.y), ('z', direction.z)]
                    .into_iter()
                    .find_map(|(name, component)| {
                        ((component.abs() - 1.0).abs() < 1e-8).then_some(name)
                    })?;
                (
                    dim.first_point,
                    dim.second_point,
                    dim.definition_point,
                    Some(axis),
                )
            }
            _ => return None,
        };
    Some(Geometry::AlignedDimension {
        start: position(start),
        end: position(end),
        dimension_line: position(location),
        axis,
        text: dimension.base().text.clone(),
    })
}

fn leader_geometry(doc: &CadDocument, leader: &Leader) -> Option<Geometry> {
    let [start, _end] = leader.vertices.as_slice() else {
        return None;
    };
    let label = doc.get_entity(leader.annotation_handle)?;
    let (label_position, text, height) = match label {
        EntityType::MText(label) => (
            label.insertion_point,
            label.value.replace(r"\P", "\n"),
            label.height,
        ),
        EntityType::Text(label) => (label.insertion_point, label.value.clone(), label.height),
        _ => return None,
    };
    Some(Geometry::Leader {
        anchor: position(*start),
        label_position: position(label_position),
        text,
        height,
    })
}

fn decode_entities<'a>(
    doc: &CadDocument,
    entities: impl Iterator<Item = &'a EntityType>,
    omitted: &mut BTreeMap<String, usize>,
) -> Vec<Record> {
    let entities: Vec<_> = entities.collect();
    let attached: std::collections::BTreeSet<_> = entities
        .iter()
        .filter_map(|entity| {
            if let EntityType::Leader(leader) = entity {
                leader_geometry(doc, leader).map(|_| leader.annotation_handle)
            } else {
                None
            }
        })
        .collect();
    let mut records = Vec::new();
    for entity in entities {
        if attached.contains(&entity.common().handle) {
            continue;
        }
        let layer = entity.as_entity().common().layer.clone();
        if let EntityType::Insert(insert) = entity {
            let canonical_name = insert.block_name.to_ascii_uppercase();
            if let Some(definition) = doc.block_records.get(&insert.block_name) {
                let base = definition.base_point;
                let points = insert.array_points();
                if !points.is_empty() {
                    for point in points {
                        let mut occurrence = insert.clone();
                        occurrence.insert_point = point;
                        let transform = occurrence.get_transform().matrix
                            * Matrix4::translation(-base.x, -base.y, -base.z);
                        records.push(Record {
                            layer: layer.clone(),
                            geometry: Geometry::Instance {
                                block: canonical_name.clone(),
                                transform: transform.m,
                            },
                        });
                    }
                    if !insert.attributes.is_empty() {
                        *omitted.entry("ATTRIB".into()).or_default() += insert.attributes.len();
                    }
                    continue;
                }
            }
            *omitted.entry("INSERT".into()).or_default() += 1;
            continue;
        }
        let geometry = match entity {
            EntityType::Line(line) => Geometry::Line {
                start: position(line.start),
                end: position(line.end),
            },
            EntityType::Point(point) => Geometry::Point {
                position: position(point.location),
            },
            EntityType::XLine(line) => Geometry::GuideLine {
                position: position(line.base_point),
                direction: position(line.direction),
            },
            EntityType::Polyline(poly) => Geometry::Polyline {
                vertices: poly
                    .vertices
                    .iter()
                    .map(|vertex| position(vertex.location))
                    .collect(),
                closed: poly.is_closed(),
                approximation: None,
            },
            EntityType::Polyline3D(poly)
                if !poly.flags.is_3d_mesh && !poly.flags.is_polyface_mesh =>
            {
                Geometry::Polyline {
                    vertices: poly
                        .vertices
                        .iter()
                        .map(|vertex| position(vertex.position))
                        .collect(),
                    closed: poly.is_closed(),
                    approximation: None,
                }
            }
            EntityType::LwPolyline(poly) => lwpolyline(poly),
            EntityType::Arc(arc) => Geometry::Polyline {
                vertices: sample_curve(arc.start_angle, arc.sweep_angle(), false, |angle| {
                    arc.point_at_angle_wcs(angle)
                }),
                closed: false,
                approximation: Some("ARC".into()),
            },
            EntityType::Circle(circle) => Geometry::Polyline {
                vertices: sample_curve(0.0, std::f64::consts::TAU, true, |angle| {
                    circle.point_at_angle_wcs(angle)
                }),
                closed: true,
                approximation: Some("CIRCLE".into()),
            },
            EntityType::Ellipse(ellipse) => {
                let minor = ellipse.normal.cross(&ellipse.major_axis) * ellipse.minor_axis_ratio;
                let closed = ellipse.is_full();
                let sweep = if closed {
                    std::f64::consts::TAU
                } else {
                    (ellipse.end_parameter - ellipse.start_parameter)
                        .rem_euclid(std::f64::consts::TAU)
                };
                Geometry::Polyline {
                    vertices: sample_curve(ellipse.start_parameter, sweep, closed, |angle| {
                        ellipse.center + ellipse.major_axis * angle.cos() + minor * angle.sin()
                    }),
                    closed,
                    approximation: Some("ELLIPSE".into()),
                }
            }
            EntityType::Face3D(face) => {
                let mut corners = vec![
                    position(face.first_corner),
                    position(face.second_corner),
                    position(face.third_corner),
                ];
                if face.fourth_corner != face.third_corner
                    && face.fourth_corner != face.first_corner
                {
                    corners.push(position(face.fourth_corner));
                }
                Geometry::Face { corners }
            }
            EntityType::Solid3D(solid) => {
                let Some(mesh) = solid.acis_data.parse().as_ref().and_then(planar_solid) else {
                    *omitted.entry("3DSOLID".into()).or_default() += 1;
                    continue;
                };
                mesh
            }
            EntityType::PolyfaceMesh(mesh) => {
                let vertices = mesh
                    .vertices
                    .iter()
                    .map(|vertex| position(vertex.location))
                    .collect::<Vec<_>>();
                let mut faces = Vec::with_capacity(mesh.faces.len());
                for face in &mesh.faces {
                    let raw = [face.index1, face.index2, face.index3, face.index4];
                    let mut ring = Vec::with_capacity(4);
                    let valid = raw[..3].iter().all(|index| *index != 0);
                    for index in raw.into_iter().filter(|index| *index != 0) {
                        let index = usize::from(index.unsigned_abs()) - 1;
                        if !ring.contains(&index) {
                            ring.push(index);
                        }
                    }
                    if !valid || ring.len() < 3 || ring.iter().any(|index| *index >= vertices.len())
                    {
                        *omitted
                            .entry("Invalid polyface mesh face".into())
                            .or_default() += 1;
                    } else {
                        faces.push(ring);
                    }
                }
                if faces.is_empty() {
                    *omitted.entry("Empty polyface mesh".into()).or_default() += 1;
                    continue;
                }
                Geometry::Mesh { vertices, faces }
            }
            EntityType::PolygonMesh(mesh) => {
                let m = usize::try_from(mesh.m_vertex_count).unwrap_or(0);
                let n = usize::try_from(mesh.n_vertex_count).unwrap_or(0);
                if m < 2 || n < 2 || m * n != mesh.vertices.len() {
                    *omitted
                        .entry("Invalid polygon mesh grid".into())
                        .or_default() += 1;
                    continue;
                }
                let mut faces = Vec::new();
                for row in 0..m - usize::from(!mesh.is_closed_m()) {
                    for col in 0..n - usize::from(!mesh.is_closed_n()) {
                        let next_row = (row + 1) % m;
                        let next_col = (col + 1) % n;
                        faces.push(vec![
                            row * n + col,
                            next_row * n + col,
                            next_row * n + next_col,
                            row * n + next_col,
                        ]);
                    }
                }
                Geometry::Mesh {
                    vertices: mesh
                        .vertices
                        .iter()
                        .map(|vertex| position(vertex.location))
                        .collect(),
                    faces,
                }
            }
            EntityType::Surface(surface) => {
                let Some(mesh) = surface.acis_data.parse().as_ref().and_then(planar_solid) else {
                    *omitted.entry("SURFACE".into()).or_default() += 1;
                    continue;
                };
                mesh
            }
            EntityType::Mesh(mesh) => Geometry::Mesh {
                vertices: mesh.vertices.iter().copied().map(position).collect(),
                faces: mesh
                    .faces
                    .iter()
                    .map(|face| face.vertices.clone())
                    .collect(),
            },
            EntityType::Dimension(dimension) => {
                let Some(geometry) = dimension_geometry(dimension) else {
                    *omitted.entry("DIMENSION".into()).or_default() += 1;
                    continue;
                };
                geometry
            }
            EntityType::Leader(leader) => {
                let Some(geometry) = leader_geometry(doc, leader) else {
                    *omitted.entry("LEADER".into()).or_default() += 1;
                    continue;
                };
                geometry
            }
            EntityType::MText(label) => Geometry::Text {
                position: position(label.insertion_point),
                text: label.value.replace(r"\P", "\n"),
                height: label.height,
                rotation: label.rotation,
            },
            EntityType::Text(label) => Geometry::Text {
                position: position(label.insertion_point),
                text: label.value.clone(),
                height: label.height,
                rotation: label.rotation,
            },
            _ => {
                *omitted
                    .entry(entity.as_entity().entity_type().to_owned())
                    .or_default() += 1;
                continue;
            }
        };
        records.push(Record { layer, geometry });
    }
    records
}

pub(super) fn encode(input: &Path, output: &Path, version: DxfVersion) -> Result<()> {
    reject_overwrite(input, output)?;
    let drawing: Drawing = serde_json::from_reader(std::io::BufReader::new(File::open(input)?))?;
    if drawing.schema != 1 {
        return Err("Unsupported BlackFox geometry exchange schema".into());
    }
    let mut doc = CadDocument::new();
    doc.version = version;
    doc.header.insertion_units = drawing.units;
    for (name, props) in drawing.layers {
        if doc.layers.get(&name).is_none() {
            doc.layers.add(Layer::new(name.as_str()))?;
        }
        let layer = doc.layers.get_mut(&name).ok_or("Layer was not created")?;
        layer.flags.off = !props.visible;
        layer.flags.locked = props.locked;
        if let Some((r, g, b)) = props.rgb {
            layer.color = Color::from_rgb(r, g, b);
        }
    }
    let mut owners = BTreeMap::new();
    for name in drawing.blocks.keys() {
        let mut definition = BlockRecord::new(name);
        definition.handle = doc.allocate_handle();
        let owner = definition.handle;
        doc.block_records.add(definition)?;
        let mut start = Block::new(name, Vector3::ZERO);
        start.common.owner_handle = owner;
        let start_handle = doc.add_entity(EntityType::Block(start))?;
        let mut end = BlockEnd::new();
        end.common.owner_handle = owner;
        let end_handle = doc.add_entity(EntityType::BlockEnd(end))?;
        let definition = doc
            .block_records
            .get_mut(name)
            .ok_or("Missing new block record")?;
        definition.block_entity_handle = start_handle;
        definition.block_end_handle = end_handle;
        owners.insert(name.clone(), owner);
    }
    for (name, records) in drawing.blocks {
        write_records(&mut doc, records, owners[&name], version)?;
    }
    let owner = doc.header.model_space_block_handle;
    write_records(&mut doc, drawing.entities, owner, version)?;
    let mut staged = temporary_output(output)?;
    let format = output_format(output)?;
    match format {
        Format::Dxf => DxfWriter::new(&doc)
            .with_version(version)
            .write_to_writer(staged.as_file_mut())?,
        Format::Dwg => DwgWriter::write_to_writer(staged.as_file_mut(), &doc)?,
        Format::Exchange(_) => unreachable!(),
    }
    staged.as_file().sync_all()?;
    let (actual, reopened) = read(staged.path())?;
    if actual != format || reopened.version != version {
        return Err("Geometry writer produced an unexpected format or version".into());
    }
    staged.persist(output)?;
    Ok(())
}

fn make_insert(block: String, rows: [[f64; 4]; 4]) -> Result<Insert> {
    let axis = |column| Vector3::new(rows[0][column], rows[1][column], rows[2][column]);
    let (x, y, z) = (axis(0), axis(1), axis(2));
    let (sx, mut sy, sz) = (x.length(), y.length(), z.length());
    if sx.min(sy).min(sz) < 1e-10 || rows[3] != [0.0, 0.0, 0.0, 1.0] {
        return Err("A CAD instance requires a nondegenerate affine transform".into());
    }
    let (x, y, z) = (x.normalize(), y.normalize(), z.normalize());
    if x.dot(&y).abs().max(x.dot(&z).abs()).max(y.dot(&z).abs()) > 1e-5 {
        return Err("A sheared instance cannot be represented by a CAD INSERT".into());
    }
    if x.cross(&y).dot(&z) < 0.0 {
        sy = -sy;
    }
    let inverse = Matrix3::arbitrary_axis(z).transpose();
    let direction = inverse.transform_point(x);
    let mut insert = Insert::new(block, inverse.transform_point(axis(3)))
        .with_scale(sx, sy, sz)
        .with_rotation(direction.y.atan2(direction.x));
    insert.normal = z;
    Ok(insert)
}

fn write_records(
    doc: &mut CadDocument,
    records: Vec<Record>,
    owner: Handle,
    version: DxfVersion,
) -> Result<()> {
    for record in records {
        if doc.layers.get(&record.layer).is_none() {
            doc.layers.add(Layer::new(record.layer.as_str()))?;
        }
        let mut entity = match record.geometry {
            Geometry::Line { start, end } => {
                EntityType::Line(Line::from_points(vector(start), vector(end)))
            }
            Geometry::Point { position } => EntityType::Point(Point::at(vector(position))),
            Geometry::GuideLine {
                position,
                direction,
            } => {
                if version == DxfVersion::AC1009 {
                    return Err("Construction lines require R13 or newer".into());
                }
                EntityType::XLine(XLine::new(vector(position), vector(direction)))
            }
            Geometry::Polyline {
                vertices, closed, ..
            } => {
                if vertices.len() < 2 {
                    return Err("A CAD polyline requires at least two vertices".into());
                }
                let mut polyline =
                    Polyline::from_points(vertices.into_iter().map(vector).collect());
                if closed {
                    polyline.close();
                }
                EntityType::Polyline(polyline)
            }
            Geometry::Face { corners } => {
                let corners = corners.into_iter().map(vector).collect::<Vec<_>>();
                match corners.as_slice() {
                    [a, b, c] => EntityType::Face3D(Face3D::triangle(*a, *b, *c)),
                    [a, b, c, d] => EntityType::Face3D(Face3D::new(*a, *b, *c, *d)),
                    _ => return Err("A CAD face requires three or four corners".into()),
                }
            }
            Geometry::BrepMesh { .. } => {
                return Err("CAD export of mesh faces with holes requires triangulation".into());
            }
            Geometry::Mesh { vertices, faces } => {
                if faces
                    .iter()
                    .any(|face| face.len() < 3 || face.iter().any(|index| *index >= vertices.len()))
                {
                    return Err("Invalid CAD mesh face indices".into());
                }
                if version < DxfVersion::AC1024 {
                    return Err("Editable MESH export requires R2010 or newer".into());
                }
                let mut mesh = Mesh::new();
                mesh.vertices = vertices.into_iter().map(vector).collect();
                mesh.faces = faces.into_iter().map(MeshFace::new).collect();
                EntityType::Mesh(mesh)
            }
            Geometry::Instance { block, transform } => {
                if doc.block_records.get(&block).is_none() {
                    return Err(format!("Missing CAD block definition {block:?}").into());
                }
                EntityType::Insert(make_insert(block, transform)?)
            }
            Geometry::AlignedDimension {
                start,
                end,
                dimension_line,
                axis,
                text,
            } => {
                let start = vector(start);
                let end = vector(end);
                let location = vector(dimension_line);
                let dimension = if let Some(axis) = axis {
                    let mut linear = match axis {
                        'x' => DimensionLinear::horizontal(start, end),
                        'y' => DimensionLinear::vertical(start, end),
                        'z' => {
                            let mut dim = DimensionLinear::vertical(start, end);
                            dim.base.normal = Vector3::UNIT_Y;
                            let direction = Matrix3::arbitrary_axis(dim.base.normal)
                                .transpose()
                                .transform_point(Vector3::UNIT_Z);
                            dim.rotation = direction.y.atan2(direction.x);
                            dim
                        }
                        _ => return Err("Unknown CAD measurement axis".into()),
                    };
                    linear.definition_point = location;
                    linear.base.definition_point = location;
                    linear.base.text = text;
                    Dimension::Linear(linear)
                } else {
                    let mut aligned = DimensionAligned::new(start, end);
                    aligned.definition_point = location;
                    aligned.base.definition_point = location;
                    aligned.base.text = text;
                    Dimension::Aligned(aligned)
                };
                EntityType::Dimension(Box::new(dimension))
            }
            Geometry::Leader {
                anchor,
                label_position,
                text,
                height,
            } => {
                if version < DxfVersion::AC1015 {
                    return Err("Text leaders require R2000 or newer in direct CAD export".into());
                }
                let mut label = MText::new();
                label.insertion_point = vector(label_position);
                label.value = text.replace('\n', r"\P");
                label.height = height;
                label.common.layer = record.layer.clone();
                label.common.owner_handle = owner;
                let mut leader = Leader::new();
                leader.vertices = vec![vector(anchor), vector(label_position)];
                leader.text_height = height;
                leader.annotation_handle = doc.add_entity(EntityType::MText(Box::new(label)))?;
                EntityType::Leader(leader)
            }
            Geometry::Text {
                position,
                text,
                height,
                rotation,
            } => {
                let mut label = Text::new();
                label.insertion_point = vector(position);
                label.value = text;
                label.height = height;
                label.rotation = rotation;
                EntityType::Text(label)
            }
        };
        entity.as_entity_mut().common_mut().layer = record.layer;
        entity.common_mut().owner_handle = owner;
        doc.add_entity(entity)?;
    }
    Ok(())
}

fn temporary_output(output: &Path) -> Result<tempfile::NamedTempFile> {
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    Ok(tempfile::NamedTempFile::new_in(parent)?)
}

fn reject_overwrite(input: &Path, output: &Path) -> Result<()> {
    use std::fs::canonicalize;
    if canonicalize(output).ok().as_ref() == Some(&canonicalize(input)?) {
        return Err("Input and destination refer to the same file".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn inspect_local_acis_fixture() {
        let input = std::env::var_os("BLACKFOX_ACIS_FIXTURE").unwrap();
        let (_, doc) = read_for_import(Path::new(&input)).unwrap();
        let mut counts = BTreeMap::new();
        for entity in doc.entities() {
            let (kind, data) = match entity {
                EntityType::Solid3D(solid) => ("solid", &solid.acis_data),
                EntityType::Surface(surface) => ("surface", &surface.acis_data),
                _ => continue,
            };
            let description = if let Some(acis) = data.parse() {
                let mut surfaces: Vec<_> = acis
                    .faces()
                    .iter()
                    .filter_map(|face| acis.resolve(face.surface()))
                    .map(|surface| surface.entity_type.as_str())
                    .collect();
                surfaces.sort_unstable();
                surfaces.dedup();
                for face in acis.faces() {
                    let Some(surface) = acis
                        .resolve(face.surface())
                        .and_then(acadrust::entities::acis::SatSplineSurface::from_record)
                    else {
                        continue;
                    };
                    let loops = acis
                        .resolve(face.first_loop())
                        .and_then(SatLoop::from_record);
                    let knots = surface.bspline(&acis).map(|net| {
                        (
                            net.degree_u,
                            net.degree_v,
                            net.control_count_u,
                            net.control_count_v,
                            net.u_closure,
                            net.v_closure,
                        )
                    });
                    println!(
                        "{kind} surface {} tokens={} net={knots:?} loops={:?}",
                        face.surface().0,
                        surface.record().tokens.len(),
                        loops.map(|l| (l.first_coedge().0, l.next_loop().0))
                    );
                }

                let mut curves = BTreeMap::new();
                for edge in acis.edges() {
                    if let Some(curve) = acis.resolve(edge.curve()) {
                        *curves.entry(curve.entity_type.as_str()).or_insert(0) += 1;
                    }
                }
                let holes = acis
                    .faces()
                    .iter()
                    .filter(|face| {
                        acis.resolve(face.first_loop())
                            .and_then(SatLoop::from_record)
                            .is_some_and(|outer| !outer.next_loop().is_null())
                    })
                    .count();
                format!(
                    "surfaces={surfaces:?}, curves={curves:?}, holes={holes}, meshed={}",
                    planar_solid(&acis).is_some()
                )
            } else {
                format!(
                    "unparsed binary={}, SAT empty={}, SAB empty={}",
                    data.is_binary,
                    data.sat_data.is_empty(),
                    data.sab_data.is_empty()
                )
            };
            *counts.entry(format!("{kind}: {description}")).or_insert(0) += 1;
        }
        println!("{counts:#?}");
    }

    #[test]
    fn planar_acis_tetrahedron_becomes_a_shared_vertex_mesh() {
        use acadrust::entities::acis::primitives::build_planar_body;
        let solid = build_planar_body(
            &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            &[vec![0, 2, 1], vec![0, 1, 3], vec![1, 2, 3], vec![2, 0, 3]],
        )
        .unwrap();
        let Some(Geometry::BrepMesh {
            vertices, faces, ..
        }) = planar_solid(&solid)
        else {
            panic!("Planar ACIS body did not produce a mesh");
        };
        assert_eq!(vertices.len(), 4);
        assert_eq!(faces.len(), 4);
        assert!(faces.iter().all(|face| face.len() == 3));
        let mut doc = CadDocument::new();
        doc.version = DxfVersion::AC1032;
        let mut entity = acadrust::entities::Solid3D::new();
        entity.set_sat_document(&solid);
        doc.add_entity(EntityType::Solid3D(entity)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        for format in ["dxf", "dwg"] {
            let source = dir.path().join(format!("solid.{format}"));
            let mut file = File::create(&source).unwrap();
            match format {
                "dxf" => DxfWriter::new(&doc).write_to_writer(&mut file).unwrap(),
                _ => DwgWriter::write_to_writer(&mut file, &doc).unwrap(),
            }
            drop(file);
            let exchange = dir.path().join("solid.json");
            decode(&source, &exchange).unwrap();
            let decoded: Drawing = serde_json::from_reader(File::open(exchange).unwrap()).unwrap();
            assert!(
                decoded.omitted.is_empty(),
                "{format}: {:?}",
                decoded.omitted
            );
            let Geometry::BrepMesh {
                vertices, faces, ..
            } = &decoded.entities[0].geometry
            else {
                panic!("{format} solid did not become a mesh");
            };
            assert_eq!((vertices.len(), faces.len()), (4, 4));
        }
    }
}
