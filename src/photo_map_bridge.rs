pub(crate) const ORIGIN: &str = "http://picsee-map.localhost/";
pub(crate) const SCHEME_HOME: &str = "picsee-map://localhost/";
const MAX_POINTS: usize = 10_000;
const MAX_IPC: usize = 50_050;

#[derive(Debug, PartialEq)]
pub(crate) enum Event {
    Ready,
    Failed,
    Select(u32, usize),
    Cluster(u32, Vec<usize>),
}

#[derive(Debug, PartialEq)]
pub(crate) enum Resource {
    Page,
    Engine,
    Points,
    Adapter,
    Css,
}

pub(crate) fn resource(method: &str, uri: &str) -> Option<Resource> {
    if method != "GET" {
        return None;
    }
    match uri
        .strip_prefix(SCHEME_HOME)
        .or_else(|| uri.strip_prefix(ORIGIN))?
    {
        "" => Some(Resource::Page),
        "engine.js" => Some(Resource::Engine),
        "points.js" => Some(Resource::Points),
        "adapter.js" => Some(Resource::Adapter),
        "map.css" => Some(Resource::Css),
        _ => None,
    }
}

pub(crate) fn parse(body: &str) -> Option<Event> {
    if body.len() > MAX_IPC {
        return None;
    }
    match body {
        "ready" => return Some(Event::Ready),
        "failed" => return Some(Event::Failed),
        _ => {}
    }
    let (kind, fields) = body.split_once(':')?;
    let (generation, payload) = fields.split_once(':')?;
    let valid_decimal =
        |value: &str| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
    let parse_id = |value: &str| {
        valid_decimal(value)
            .then(|| value.parse::<usize>().ok())
            .flatten()
            .filter(|n| *n < MAX_POINTS)
    };
    if !valid_decimal(generation) {
        return None;
    }
    let generation = generation.parse::<u32>().ok().filter(|n| *n > 0)?;
    match kind {
        "select" => Some(Event::Select(generation, parse_id(payload)?)),
        "cluster" => {
            let mut seen = std::collections::HashSet::new();
            let mut ids = Vec::new();
            for part in payload.split(',') {
                let id = parse_id(part)?;
                if !seen.insert(id) {
                    return None;
                }
                ids.push(id);
            }
            (ids.len() > 1).then_some(Event::Cluster(generation, ids))
        }
        _ => None,
    }
}

pub(crate) fn route(event: Event, generation: u32, len: usize) -> Option<Vec<usize>> {
    let (incoming, ids) = match event {
        Event::Select(generation, id) => (generation, vec![id]),
        Event::Cluster(generation, ids) => (generation, ids),
        _ => return None,
    };
    (incoming == generation && ids.iter().all(|id| *id < len)).then_some(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_bounded_messages() {
        assert_eq!(parse("ready"), Some(Event::Ready));
        assert_eq!(parse("failed"), Some(Event::Failed));
        assert_eq!(parse("select:7:0"), Some(Event::Select(7, 0)));
        assert_eq!(parse("cluster:7:9,2"), Some(Event::Cluster(7, vec![9, 2])));
        for invalid in [
            "select:7:D:\\photo.jpg",
            "select:7:10000",
            "select:7:+1",
            "select:7:1\n",
            "select:7:1:2",
            "select:0:1",
            "cluster:7:",
            "cluster:7:1",
            "cluster:7:1,1",
            "cluster:7:1,10000",
            "open:7:1",
            "ready:",
        ] {
            assert_eq!(parse(invalid), None, "{invalid:?}");
        }
        assert_eq!(parse(&"x".repeat(MAX_IPC + 1)), None);
    }

    #[test]
    fn stale_and_unknown_ids_are_never_dispatched() {
        assert_eq!(route(Event::Select(7, 0), 8, 10), None);
        assert_eq!(route(Event::Select(8, 10), 8, 10), None);
        assert_eq!(route(Event::Select(8, 9), 8, 10), Some(vec![9]));
        assert_eq!(route(Event::Cluster(8, vec![9, 10]), 8, 10), None);
        assert_eq!(
            route(Event::Cluster(8, vec![9, 2]), 8, 10),
            Some(vec![9, 2])
        );
    }

    #[test]
    fn only_exact_local_resources_are_served() {
        assert_eq!(
            resource("GET", "http://picsee-map.localhost/"),
            Some(Resource::Page)
        );
        assert_eq!(
            resource("GET", "http://picsee-map.localhost/engine.js"),
            Some(Resource::Engine)
        );
        assert_eq!(
            resource("GET", "picsee-map://localhost/points.js"),
            Some(Resource::Points)
        );
        for (verb, uri) in [
            ("POST", "http://picsee-map.localhost/"),
            ("GET", "https://picsee-map.localhost/"),
            ("GET", "http://picsee-map.localhost.evil.test/"),
            ("GET", "http://picsee-map.localhost/engine.js?q=1"),
            ("GET", "http://picsee-map.localhost/%2e%2e/engine.js"),
            ("GET", "file:///D:/photos/secret.jpg"),
            ("GET", "http://picsee-map.localhost/../engine.js"),
        ] {
            assert_eq!(resource(verb, uri), None);
        }
    }
}
