// What Symfony and Doctrine make available at runtime that their classes don't declare, after what
// phpstan-symfony and phpstan-doctrine do for PHPStan: services from the container by id or class,
// parameters with the type of their value, an entity's own repository, and a repository's magic
// `findBy*`, `findOneBy*` and `countBy*` methods.

fn return_type_targets() {
    #{
        methods: [
            "Psr\\Container\\ContainerInterface::get",
            "Symfony\\Component\\DependencyInjection\\ContainerInterface::get",
            "Symfony\\Bundle\\FrameworkBundle\\Controller\\AbstractController::getParameter",
            "Symfony\\Component\\DependencyInjection\\ParameterBag\\ParameterBagInterface::get",
            "Symfony\\Component\\DependencyInjection\\ContainerInterface::getParameter",
            "Doctrine\\Persistence\\ObjectManager::getRepository",
            "Doctrine\\ORM\\EntityManagerInterface::getRepository",
            "Doctrine\\Persistence\\ManagerRegistry::getRepository",
        ],
    }
}

fn return_type_of_method(node, ctx) {
    if node.arguments.len() == 0 { return; }
    let arg = ctx.get_type(node.arguments[0]);
    let name = node.name;
    if name == "getRepository" { return repository_of(arg.class_string); }
    if name == "get" {
        // a container's service: by its class, or by its id
        let class_name = arg.class_string;
        if class_name != () { return class_name; }
        let id = arg.literal;
        if type_of(id) != "string" { return; }
        let found = symfony_service(id);
        if found != () && class(found) != () { return class(found).name; }
        return;
    }
    if name == "getParameter" {
        let key = arg.literal;
        if type_of(key) != "string" { return; }
        let found = symfony_parameter(key);
        if !found.found { return; }
        return type_of_value(found.value);
    }
}

// The PHPDoc type of a parameter's value: not its literal value, as that's for the environment it
// was compiled for.
fn type_of_value(value) {
    let t = type_of(value);
    if t == "()" { return "null"; }
    if t == "bool" { return "bool"; }
    if t == "i64" { return "int"; }
    if t == "f64" { return "float"; }
    if t == "string" { return "string"; }
    if t == "array" { return "list<mixed>"; }
    if t == "map" { return "array<string, mixed>"; }
}

// The repository an entity says it has (`#[ORM\Entity(repositoryClass: ..)]`).
fn repository_of(entity) {
    if entity == () { return; }
    let c = class(entity);
    if c == () { return; }
    for a in c.attributes() {
        if a.name == "Doctrine\\ORM\\Mapping\\Entity" {
            let repository = a.args["repositoryClass"];
            if repository == () { repository = a.args["0"]; }
            if type_of(repository) == "string" && class(repository) != () {
                return class(repository).name;
            }
        }
    }
}

// `findByEmail($email)`, `findOneByEmail($email)` and `countByEmail($email)` on a repository.
fn magic_method(type, name, is_static) {
    let class_name = type.class_name;
    if class_name == () || is_static { return; }
    let prefix = "";
    for p in ["findOneBy", "findBy", "countBy"] {
        if name.len() > p.len() && name.starts_with(p) { prefix = p; break; }
    }
    if prefix == "" { return; }
    let c = class(class_name);
    if c == () || !c.is_subtype_of("Doctrine\\ORM\\EntityRepository") { return; }
    if prefix == "countBy" { return "int"; }
    // the entity is what `find()` gives
    let entity = type.method_type("find");
    if entity == () || entity.is_unknown() { return "mixed"; }
    let entity = entity.without_null().to_string();
    if prefix == "findOneBy" { return "?" + entity; }
    "list<" + entity + ">"
}
