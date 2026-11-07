// What Laravel makes available at runtime that its classes don't declare, after what larastan does
// for PHPStan: what Eloquent models get from their casts, relations, accessors and scopes, calls
// forwarded from models and relations to their query builder, what facades and contracts stand for,
// and the user model of `auth()->user()`.

fn model_class() { "Illuminate\\Database\\Eloquent\\Model" }
fn eloquent_builder() { "Illuminate\\Database\\Eloquent\\Builder" }
fn relation_class() { "Illuminate\\Database\\Eloquent\\Relations\\Relation" }

// The classes Laravel's container binds to its contracts and service names
// (`Application::registerCoreContainerAliases`), for facades and contracts.
fn bound(name) {
    let names = #{
        "app": "Illuminate\\Foundation\\Application",
        "auth": "Illuminate\\Auth\\AuthManager",
        "auth.driver": "Illuminate\\Auth\\SessionGuard",
        "auth.password": "Illuminate\\Auth\\Passwords\\PasswordBrokerManager",
        "auth.password.broker": "Illuminate\\Auth\\Passwords\\PasswordBroker",
        "blade.compiler": "Illuminate\\View\\Compilers\\BladeCompiler",
        "cache": "Illuminate\\Cache\\CacheManager",
        "cache.store": "Illuminate\\Cache\\Repository",
        "config": "Illuminate\\Config\\Repository",
        "cookie": "Illuminate\\Cookie\\CookieJar",
        "db": "Illuminate\\Database\\DatabaseManager",
        "db.connection": "Illuminate\\Database\\Connection",
        "db.schema": "Illuminate\\Database\\Schema\\Builder",
        "encrypter": "Illuminate\\Encryption\\Encrypter",
        "events": "Illuminate\\Events\\Dispatcher",
        "files": "Illuminate\\Filesystem\\Filesystem",
        "filesystem": "Illuminate\\Filesystem\\FilesystemManager",
        "filesystem.disk": "Illuminate\\Filesystem\\FilesystemAdapter",
        "filesystem.cloud": "Illuminate\\Filesystem\\FilesystemAdapter",
        "hash": "Illuminate\\Hashing\\HashManager",
        "log": "Illuminate\\Log\\LogManager",
        "mail.manager": "Illuminate\\Mail\\MailManager",
        "mailer": "Illuminate\\Mail\\Mailer",
        "queue": "Illuminate\\Queue\\QueueManager",
        "redirect": "Illuminate\\Routing\\Redirector",
        "redis": "Illuminate\\Redis\\RedisManager",
        "request": "Illuminate\\Http\\Request",
        "router": "Illuminate\\Routing\\Router",
        "session": "Illuminate\\Session\\SessionManager",
        "session.store": "Illuminate\\Session\\Store",
        "translator": "Illuminate\\Translation\\Translator",
        "url": "Illuminate\\Routing\\UrlGenerator",
        "validator": "Illuminate\\Validation\\Factory",
        "view": "Illuminate\\View\\Factory",
    };
    names[name]
}

// The class a contract is bound to, as `bound` has it.
fn contract_binding(name) {
    let contracts = #{
        "Illuminate\\Contracts\\Auth\\Factory": "auth",
        "Illuminate\\Contracts\\Auth\\Guard": "auth.driver",
        "Illuminate\\Contracts\\Auth\\StatefulGuard": "auth.driver",
        "Illuminate\\Contracts\\Auth\\PasswordBroker": "auth.password.broker",
        "Illuminate\\Contracts\\Cache\\Factory": "cache",
        "Illuminate\\Contracts\\Cache\\Repository": "cache.store",
        "Illuminate\\Contracts\\Config\\Repository": "config",
        "Illuminate\\Contracts\\Cookie\\Factory": "cookie",
        "Illuminate\\Contracts\\Cookie\\QueueingFactory": "cookie",
        "Illuminate\\Contracts\\Encryption\\Encrypter": "encrypter",
        "Illuminate\\Contracts\\Events\\Dispatcher": "events",
        "Illuminate\\Contracts\\Filesystem\\Factory": "filesystem",
        "Illuminate\\Contracts\\Filesystem\\Filesystem": "filesystem.disk",
        "Illuminate\\Contracts\\Filesystem\\Cloud": "filesystem.cloud",
        "Illuminate\\Contracts\\Mail\\Factory": "mail.manager",
        "Illuminate\\Contracts\\Mail\\Mailer": "mailer",
        "Illuminate\\Contracts\\Queue\\Factory": "queue",
        "Illuminate\\Contracts\\Redis\\Factory": "redis",
        "Illuminate\\Contracts\\Routing\\Registrar": "router",
        "Illuminate\\Contracts\\Routing\\UrlGenerator": "url",
        "Illuminate\\Contracts\\Session\\Session": "session.store",
        "Illuminate\\Contracts\\Translation\\Translator": "translator",
        "Illuminate\\Contracts\\Validation\\Factory": "validator",
        "Illuminate\\Contracts\\View\\Factory": "view",
        "Illuminate\\Contracts\\Foundation\\Application": "app",
        "Illuminate\\Contracts\\Container\\Container": "app",
    };
    let service = contracts[name];
    if service == () { return; }
    let to = bound(service);
    // only what the project has: an application without that package has no such binding
    if class(to) == () { return; }
    to
}

fn magic_members(name) {
    let c = class(name);
    if c == () { return; }
    if c.kind == "interface" {
        let to = contract_binding(name);
        if to != () { return #{ forward_calls: to }; }
        return;
    }
    if c.kind != "class" { return; }
    if name != model_class() && c.is_subtype_of(model_class()) {
        return model_members(c);
    }
    if name != "Illuminate\\Support\\Facades\\Facade" && c.is_subtype_of("Illuminate\\Support\\Facades\\Facade") {
        return facade_members(c);
    }
    // what the default guard has, the manager has
    if name == "Illuminate\\Auth\\AuthManager" && class("Illuminate\\Auth\\SessionGuard") != () {
        return #{ forward_calls: "Illuminate\\Auth\\SessionGuard" };
    }
}

// --------------------------------------------------------------------------------------- facades

// A facade's static calls go to what its accessor names in the container.
fn facade_members(c) {
    let accessor = c.method("getFacadeAccessor");
    if accessor == () { return; }
    let named = accessor.returned_value();
    if type_of(named) != "string" { return; }
    // a service's name first: `url` isn't the class `URL`
    let to = bound(named);
    if to == () && class(named) != () { to = class(named).name; }
    if to == () || class(to) == () || class(to).is_subtype_of("Illuminate\\Support\\Facades\\Facade") { return; }
    #{ forward_calls: to }
}

// ---------------------------------------------------------------------------------------- models

// Declared by Eloquent itself, on `Model` and the traits it's made of, rather than by a model.
fn from_eloquent(m) {
    m.class.starts_with("Illuminate\\Database\\Eloquent\\")
}

fn model_members(c) {
    let properties = #{};
    let methods = #{};
    let model = c.name;

    // what the model itself declares, rather than Eloquent
    let own = c.methods_below(model_class());

    for m in own {
        if m.is_static || m.is_magic || from_eloquent(m) { continue; }
        let name = m.name;
        // relations: a property of the same name holds what they get
        if m.visibility == "public" {
            let result = relation_result(m);
            if result != () {
                properties[name] = #{ type: result, read_only: true, declared_at: m.class + "::" + name };
                continue;
            }
        }
        // accessors: `getFirstNameAttribute()` and `firstName(): Attribute` are `first_name`
        if name.len() > 12 && name.starts_with("get") && name.ends_with("Attribute") {
            let attribute = snake(name.sub_string(3, name.len() - 12));
            if attribute != "" && !properties.contains(attribute) {
                let t = m.method.return_type;
                let ty = if t.is_unknown() { "mixed" } else { t.to_string() };
                properties[attribute] = #{ type: ty, write: "mixed", declared_at: m.class + "::" + name };
            }
        } else if m.returns_class == "Illuminate\\Database\\Eloquent\\Casts\\Attribute" {
            let attribute = snake(name);
            if !properties.contains(attribute) {
                let ty = "mixed";
                if m.returns_generic {
                    let args = m.method.return_type.type_args;
                    if args.len() > 0 && !args[0].is_unknown() { ty = args[0].to_string(); }
                }
                properties[attribute] = #{ type: ty, write: "mixed", declared_at: m.class + "::" + name };
            }
        }
    }

    // the columns of the model's table, where its migrations say what they are
    let table = c.property("table");
    let table = if table != () && type_of(table.default_value) == "string" { table.default_value } else { () };
    let columns = #{};
    for column in laravel_columns(model, table) {
        columns[column.name] = column;
    }

    // casts, from `$casts` and from `casts()`: a column that can be null still can
    let casts = #{};
    let declared = c.property("casts");
    if declared != () {
        let value = declared.default_value;
        if type_of(value) == "map" { casts += value; }
    }
    let method = c.method("casts");
    if method != () && !from_eloquent(method) {
        let value = method.returned_value();
        if type_of(value) == "map" { casts += value; }
    }
    for name in casts.keys() {
        let ty = cast_type(casts[name]);
        if ty != () && !properties.contains(name) {
            let column = columns[name];
            let nullable = column != () && column.nullable && ty != "mixed";
            // Laravel casts what's set, too
            properties[name] = #{ type: if nullable { "?" + ty } else { ty }, write: "mixed" };
        }
    }

    for name in columns.keys() {
        if !properties.contains(name) {
            properties[name] = #{ type: columns[name].type, write: "mixed" };
        }
    }

    // scopes: `scopeActive(Builder $query)`, or `#[Scope] active(Builder $query)`
    for m in own {
        if m.is_magic || from_eloquent(m) { continue; }
        let scope = scope_name(m);
        if scope != () && !methods.contains(scope) {
            methods[scope] = #{
                returns: eloquent_builder() + "<static>",
                params: m.method.params_text(1),
                is_static: true,
                declared_at: m.class + "::" + m.name,
            };
        }
    }

    #{ properties: properties, methods: methods, forward_calls: eloquent_builder() + "<static>" }
}

// The scope a method is, as it's called: `active` for `scopeActive` and for `#[Scope] active`.
// `m` is what `method_details` says of it.
fn scope_name(m) {
    let name = m.name;
    if name.len() > 5 && name.starts_with("scope") {
        let first = name.sub_string(5, 1);
        if first.to_upper() == first {
            return first.to_lower() + name.sub_string(6);
        }
    }
    for a in m.attributes {
        if a.name == "Illuminate\\Database\\Eloquent\\Attributes\\Scope" { return name; }
    }
}

// What a relation method's property holds, as a PHPDoc type: what `getResults()` gives. `m` is
// what `method_details` says of it.
fn relation_result(m) {
    if m.required_params > 0 { return; }
    // what the call it returns says is related, when the type says no more than the class
    let made = m.returned_call;
    if made != () && !m.returns_generic {
        let many = ["hasMany", "morphMany", "belongsToMany", "morphToMany", "morphedByMany", "hasManyThrough"];
        let one = ["hasOne", "belongsTo", "morphOne", "hasOneThrough"];
        let is_many = made.name in many;
        if is_many || made.name in one {
            let related = made.args["0"];
            let related = if type_of(related) == "string" { class(related) } else { () };
            if related == () { return "mixed"; }
            if is_many { return "Illuminate\\Database\\Eloquent\\Collection<int, " + related.name + ">"; }
            return "?" + related.name;
        }
        if made.name == "morphTo" { return "?" + model_class(); }
    }
    if m.returns_class == () { return; }
    let relation = class(m.returns_class);
    if relation == () || !relation.is_subtype_of(relation_class()) { return; }
    let t = m.method.return_type.method_type("getResults");
    if t != () && !t.is_unknown() { return t.to_string(); }
    "mixed"
}

// The type a cast gives an attribute.
fn cast_type(cast) {
    if type_of(cast) != "string" { return; }
    let name = cast;
    let colon = cast.index_of(":");
    if colon > 0 { name = cast.sub_string(0, colon); }
    let types = #{
        "int": "int", "integer": "int", "timestamp": "int",
        "real": "float", "float": "float", "double": "float",
        "decimal": "numeric-string",
        "string": "string", "hashed": "string", "encrypted": "string",
        "bool": "bool", "boolean": "bool",
        "object": "\\stdClass",
        "array": "array<array-key, mixed>", "json": "array<array-key, mixed>",
        "collection": "\\Illuminate\\Support\\Collection<array-key, mixed>",
        "date": "\\Illuminate\\Support\\Carbon", "datetime": "\\Illuminate\\Support\\Carbon",
        "custom_datetime": "\\Illuminate\\Support\\Carbon",
        "immutable_date": "\\Carbon\\CarbonImmutable", "immutable_datetime": "\\Carbon\\CarbonImmutable",
        "immutable_custom_datetime": "\\Carbon\\CarbonImmutable",
    };
    let known = types[name.to_lower()];
    if known != () {
        if name.to_lower() == "encrypted" && colon > 0 {
            return cast_type(cast.sub_string(colon + 1));
        }
        return known;
    }
    let c = class(name);
    if c == () { return "mixed"; }
    if c.kind == "enum" { return c.name; }
    if c.is_subtype_of("Illuminate\\Database\\Eloquent\\Casts\\AsCollection") {
        return "\\Illuminate\\Support\\Collection<array-key, mixed>";
    }
    if c.is_subtype_of("Illuminate\\Database\\Eloquent\\Casts\\AsArrayObject") {
        return "\\Illuminate\\Database\\Eloquent\\Casts\\ArrayObject<array-key, mixed>";
    }
    if c.is_subtype_of("Illuminate\\Database\\Eloquent\\Casts\\AsStringable") {
        return "\\Illuminate\\Support\\Stringable";
    }
    // a cast class: what its `get()` gives
    let get = c.method("get");
    if get != () && c.is_subtype_of("Illuminate\\Contracts\\Database\\Eloquent\\CastsAttributes") {
        let t = get.return_type;
        if !t.is_unknown() && t.to_string() != "TGet" { return t.to_string(); }
    }
    // a value object that says how it's cast
    "mixed"
}

// `firstName` and `FirstName` are `first_name`.
fn snake(name) {
    let out = "";
    let i = 0;
    for ch in name.chars() {
        let s = ch.to_string();
        if s.to_upper() == s && s.to_lower() != s {
            if i > 0 { out += "_"; }
            out += s.to_lower();
        } else {
            out += s;
        }
        i += 1;
    }
    out
}

// ----------------------------------------------------------------------------- one name at a time

fn magic_method(type, name, is_static) {
    let class_name = type.class_name;
    if class_name == () { return; }
    if class_name == eloquent_builder() {
        return builder_method(type, name);
    }
    let c = class(class_name);
    if c == () { return; }
    // a relation forwards what it hasn't got to a query of its related models, and gives back
    // itself where that would be the query
    if c.is_subtype_of(relation_class()) {
        let args = type.type_args;
        let related = if args.len() > 0 && !args[0].is_unknown() { args[0].to_string() } else { model_class() };
        return #{ forward_to: eloquent_builder() + "<" + related + ">", fluent: true };
    }
    if class_name == "Illuminate\\Database\\Query\\Builder" && is_dynamic_where(name) {
        return "$this";
    }
    // a macro the project adds, `Collection::macro('toUpper', ..)`: what it is isn't followed
    if c.is_subtype_of("Illuminate\\Support\\Traits\\Macroable") {
        for ancestor in c.ancestors() {
            if name in laravel_macros(ancestor) { return "mixed"; }
        }
    }
    // `$query->orWhere->active()`: a scope of whichever model it's a query of
    if class_name == "Illuminate\\Database\\Eloquent\\HigherOrderBuilderProxy" {
        return "mixed";
    }
}

// What a query of models has from the model: its scopes, and the query methods it declares itself,
// such as `withTrashed` for one that's soft deleted.
fn builder_method(type, name) {
    let args = type.type_args;
    if args.len() > 0 {
        // of each model a query can be of, for `Builder<Page|Chapter>`
        let found = ();
        for model in args[0].types() {
            let answer = model_query_method(model.class_name, name);
            if answer == () { found = (); break; }
            if found == () { found = answer; }
        }
        if found != () { return found; }
    }
    if is_dynamic_where(name) { return "$this"; }
}

fn model_query_method(model_name, name) {
    if model_name == () { return; }
    let model = class(model_name);
    if model == () { return; }
    let own = [];
    let candidates = [];
    for m in model.methods() {
        if m.is_magic || from_eloquent(m) { continue; }
        // what can be a scope by that name: `scopeActive`, or `active` itself with `#[Scope]`
        let lower = m.name.to_lower();
        if lower == "scope" + name.to_lower() || lower == name.to_lower() { candidates.push(m.name); }
    }
    for m in model.method_details(candidates) {
        if scope_name(m) == name {
            return #{ returns: "$this", params: m.method.params_text(1), declared_at: m.class + "::" + m.name };
        }
    }
    let own = model.method(name);
    if own != () && own.is_static {
        let t = own.return_type;
        if !t.is_unknown() && t.class_name == eloquent_builder() {
            return #{ returns: "$this", params: own.params_text(0) };
        }
    }
}

// `whereEmail($email)`, which Laravel turns into `where('email', $email)`.
fn is_dynamic_where(name) {
    if name.len() <= 5 || !name.starts_with("where") { return false; }
    let next = name.sub_string(5, 1);
    next.to_upper() == next && next.to_lower() != next
}

// An attribute a model hasn't got a declaration for: whatever is in that column, if the database
// has it. Its migrations, if there are any, aren't the whole story.
fn magic_property(type, name) {
    let class_name = type.class_name;
    if class_name == () { return; }
    if class_name == model_class() { return "mixed"; }
    let c = class(class_name);
    if c == () { return; }
    if c.is_subtype_of(model_class()) { return "mixed"; }
    // a request's input, by name
    if c.is_subtype_of("Illuminate\\Http\\Request") { return "mixed"; }
    // `$users->map->name`: each value's, as the values have it
    if class_name == "Illuminate\\Support\\HigherOrderCollectionProxy" {
        let args = type.type_args;
        if args.len() > 1 && args[1].class_name != () { return #{ forward_to: args[1].to_string() }; }
    }
}

// ----------------------------------------------------------------------------- return types

fn return_type_targets() {
    #{
        methods: [
            "Illuminate\\Contracts\\Auth\\Guard::user",
            "Illuminate\\Auth\\SessionGuard::user",
            "Illuminate\\Auth\\AuthManager::user",
            "Illuminate\\Http\\Request::user",
        ],
        static_methods: ["Illuminate\\Support\\Facades\\Auth::user"],
    }
}

// The user model of the project's configuration, for what gives the user signed in.
fn user_model() {
    let configured = laravel_config("auth.providers.users.model");
    if type_of(configured) == "string" && class(configured) != () { return class(configured).name; }
    for name in ["App\\Models\\User", "App\\User"] {
        if class(name) != () { return name; }
    }
}

fn return_type_of_method(node, ctx) {
    let user = user_model();
    if user == () { return; }
    "?" + user
}

fn return_type_of_static_method(node, ctx) {
    let user = user_model();
    if user == () { return; }
    "?" + user
}
