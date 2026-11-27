// Negative distances are inside the glass. The empty area is an open
// quadrant with one rounded corner carved out of the outer silhouette.
fn corner_distance(q: vec2<f32>, radius: f32) -> f32 {
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn shell_distance(pixel: vec2<f32>, size: vec2<f32>, dims: vec4<f32>, junction: f32) -> f32 {
    let inset = dims.x;
    let p = pixel - vec2<f32>(inset);
    let area = max(size - 2.0 * inset, vec2<f32>(1.0));
    if dims.y <= 0.0 {
        if dims.z <= 0.0 { return 1.0e6; }
        let half_extent = vec2<f32>(area.x, dims.z) * 0.5;
        let center = vec2<f32>(half_extent.x, area.y - half_extent.y);
        return sd_rounded_rect(p - center, half_extent, vec4<f32>(min(dims.w, half_extent.y)));
    }
    let radius = min(dims.w, min(dims.y, dims.z) * 0.5);
    let top_of_player = area.y - dims.z;
    let inner = clamp(junction, 0.0, min(area.x - dims.y, top_of_player));
    let outside = sd_rounded_rect(p - area * 0.5, area * 0.5, vec4<f32>(radius));
    let opening = corner_distance(
        vec2<f32>(dims.y + inner - p.x, p.y - top_of_player + inner),
        inner,
    );
    var distance = max(outside, -opening);

    // Round the two exposed ends. The bottom-left and bottom-right corners
    // are already part of the outer rounded rectangle.
    if p.y < radius {
        let sidebar_cap = p - vec2<f32>(dims.y - radius, radius);
        distance = max(distance, corner_distance(vec2<f32>(sidebar_cap.x, -sidebar_cap.y), radius));
    }
    if p.x > area.x - radius {
        let bar_cap = p - vec2<f32>(area.x - radius, top_of_player + radius);
        distance = max(distance, corner_distance(vec2<f32>(bar_cap.x, -bar_cap.y), radius));
    }
    return distance;
}

// The right upright extends to the same bottom contour as the player. Its
// concave opening is the mirror of the menu elbow, so the union has no seam.
fn dock_distance(pixel: vec2<f32>, size: vec2<f32>, dims: vec4<f32>, width: f32, junction: f32) -> f32 {
    let p = pixel - vec2<f32>(dims.x);
    let area = size - vec2<f32>(2.0 * dims.x);
    let player = area.y - dims.z;
    if width <= 0.0 || player <= TITLEBAR_HEIGHT {
        return 1.0e6;
    }
    let dock_left = area.x - width;
    let cap = min(dims.w * 0.5, width * 0.5);
    let inner = clamp(junction, 0.0, min((dock_left - dims.y) * 0.5, player - TITLEBAR_HEIGHT - cap));
    let outer_radius = min(dims.w, min(dims.y, dims.z) * 0.5);
    let outside = sd_rounded_rect(p - area * 0.5, area * 0.5, vec4<f32>(outer_radius));
    let opening = corner_distance(vec2<f32>(p.x - dock_left + inner, p.y - player + inner), inner);
    let top_cap = corner_distance(
        vec2<f32>(abs(p.x - (area.x - width * 0.5)) - (width * 0.5 - cap), TITLEBAR_HEIGHT + cap - p.y),
        cap,
    );
    // Apply the upper cap only at the top. Below it the inner elbow can
    // travel left of the upright to meet the horizontal player edge.
    let top = select(TITLEBAR_HEIGHT - p.y, top_cap, p.y < TITLEBAR_HEIGHT + cap);
    return max(max(outside, -opening), top);
}
