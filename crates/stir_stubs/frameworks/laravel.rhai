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

// ----------------------------------------------------------------------------------- factories

fn factory_class() { "Illuminate\\Database\\Eloquent\\Factories\\Factory" }

// The factory of a model, as `HasFactory::factory()` finds it: `$factory`, `#[UseFactory]`, then
// by name (`Factory::resolveFactoryName()`), as nothing declares it without `@use`.
fn factory_of(model) {
    let found = ();
    let declared = model.property("factory");
    if declared != () && type_of(declared.default_value) == "string" { found = class(declared.default_value); }
    if found == () {
        for a in model.attributes() {
            if a.name == "Illuminate\\Database\\Eloquent\\Attributes\\UseFactory" {
                let named = a.args["0"];
                if type_of(named) == "string" { found = class(named); }
            }
        }
    }
    if found == () {
        // `newFactory()` a model has itself, which is usually just this
        let method = model.method("newFactory");
        if method != () && !from_eloquent(method) {
            let t = method.return_type;
            if t.class_name != () && t.class_name != factory_class() { found = class(t.class_name); }
            if found == () {
                let named = method.returned_class();
                if named != () { found = class(named); }
            }
        }
    }
    if found == () {
        let name = model.name;
        for app in ["App\\Models\\", "App\\"] {
            if name.starts_with(app) {
                name = name.sub_string(app.len());
                break;
            }
        }
        found = class("Database\\Factories\\" + name + "Factory");
    }
    if found != () && found.is_subtype_of(factory_class()) { found }
}

// The model of a factory: what its `@extends` says, `$model`, then by name (`Factory::modelName()`).
fn model_of_factory(factory) {
    let made = factory.type.method_type("newModel");
    if made != () && made.class_name != () && made.class_name != model_class() { return class(made.class_name); }
    let declared = factory.property("model");
    if declared != () && type_of(declared.default_value) == "string" {
        let model = class(declared.default_value);
        if model != () { return model; }
    }
    let name = factory.name;
    if !name.ends_with("Factory") { return; }
    name = name.sub_string(0, name.len() - 7);
    if name.starts_with("Database\\Factories\\") { name = name.sub_string(19); }
    for app in ["App\\Models\\", "App\\"] {
        let model = class(app + name);
        if model != () { return model; }
    }
}

// Has a `count()` been asked for on the way to this factory, as far as it can be seen? One that's
// been passed around is taken to have none, as most haven't.
fn makes_many(node) {
    let at = node.receiver;
    while at != () {
        let args = at.arguments;
        if at.kind == "method_call" && (at.name == "count" || at.name == "times") {
            return args.len() > 0 && args[0].text.to_lower() != "null";
        }
        if at.kind == "static_call" && at.name == "times" { return true; }
        if at.kind == "static_call" && at.name == "factory" {
            // a number, rather than the state that can be given here too
            if args.len() == 0 { return false; }
            let first = args[0];
            return !(first.kind in ["array", "closure", "arrow_function"]) && first.text.to_lower() != "null";
        }
        if at.kind != "method_call" { return false; }
        at = at.receiver;
    }
    false
}

// What a factory's method that makes models gives: its `create()` and `make()` say either a model
// or a collection of them, for it depends on `count()`.
fn factory_result(node, factory) {
    let model = model_of_factory(factory);
    if model == () { return; }
    let many = "Illuminate\\Database\\Eloquent\\Collection<int, " + model.name + ">";
    let name = node.name.to_lower();
    if name in ["createone", "makeone", "createonequietly"] { return model.name; }
    if name in ["createmany", "createmanyquietly"] { return many; }
    if name in ["create", "make", "createquietly"] {
        return if makes_many(node) { many } else { model.name };
    }
}

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

// What a relation method says it returns, with what its body says is related, when the method says
// just the class: `articles(): HasMany` with `return $this->hasMany(Article::class)`.
fn relation_with_models(c, name, relation) {
    let found = c.method_details([name]);
    if found.len() == 0 { return; }
    let made = found[0].returned_call;
    if made == () { return; }
    let related = made.args["0"];
    let related = if type_of(related) == "string" { class(related) } else { () };
    if related == () { return; }
    let through = made.args["1"];
    let args = [];
    for param in relation.type_params {
        if param == "TRelatedModel" {
            args.push(related.name);
        } else if param == "TDeclaringModel" {
            args.push("$this");
        } else if param == "TIntermediateModel" && type_of(through) == "string" && class(through) != () {
            args.push(class(through).name);
        } else if param == "TIntermediateModel" {
            args.push(model_class());
        } else {
            // what comes after has a default
            break;
        }
    }
    if args.len() == 0 { return; }
    relation.name + "<" + args.reduce(|sum, v| if sum == () { v } else { sum + ", " + v }) + ">"
}

// What a model's method says it returns, when it says a collection and not of what, with what its
// body gets it from: `replies(): Collection` with `return $this->repliesRelation;`, or
// `return $this->repliesRelation()->latest()->get();`.
fn collection_of_relation(c, name) {
    let found = c.method_details([name]);
    if found.len() == 0 { return; }
    let relation = found[0].method.returned_property();
    if relation == () {
        let made = found[0].returned_call;
        if made != () { relation = made.name; }
    }
    if relation == () { return; }
    let from = c.method_details([relation]);
    if from.len() == 0 { return; }
    let result = relation_result(from[0]);
    if type_of(result) == "string" && result.starts_with("Illuminate\\Database\\Eloquent\\Collection<") {
        return result;
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
    if class_name == "Illuminate\\Support\\Optional" {
        // given something, it calls the method on it, and gives `null` for what isn't an object
        let args = type.type_args;
        if args.len() > 0 && !args[0].is_unknown() {
            return #{ forward_to: args[0].to_string(), nullable: true };
        }
        return "mixed";
    }
    if class_name == "Illuminate\\Support\\HigherOrderTapProxy" {
        // whatever its method gives, what comes back is what's being tapped
        let args = type.type_args;
        if args.len() > 0 && args[0].has_method(name) { return args[0].to_string(); }
        return;
    }
    if c.is_subtype_of(factory_class()) && !is_static {
        let found = factory_magic(c, name);
        if found != () { return found; }
    }
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
    // a macro the project or a package adds, `Collection::macro('toUpper', ..)`: what it is isn't
    // followed
    if c.is_subtype_of("Illuminate\\Support\\Traits\\Macroable") {
        for ancestor in c.ancestors() {
            if name in laravel_macros(ancestor) { return "mixed"; }
        }
    }
    // on a facade, it's on what's behind it, and can be added through it as well
    if c.is_subtype_of("Illuminate\\Support\\Facades\\Facade") {
        if name in laravel_macros(class_name) { return "mixed"; }
        let members = facade_members(c);
        if members != () && class(members.forward_calls) != () {
            for ancestor in class(members.forward_calls).ancestors() {
                if name in laravel_macros(ancestor) { return "mixed"; }
            }
        }
    }
    // `$query->orWhere->active()`: a scope of whichever model it's a query of
    if class_name == "Illuminate\\Database\\Eloquent\\HigherOrderBuilderProxy" {
        return "mixed";
    }
}

// `hasPosts()` and `forUser()` on a factory make the related models, if the model has that relation.
fn factory_magic(factory, name) {
    if name == "trashed" { return "$this"; }
    if name.len() <= 3 || !(name.starts_with("has") || name.starts_with("for")) { return; }
    let model = model_of_factory(factory);
    if model == () { return "$this"; }
    let relation = name.sub_string(3, 1).to_lower() + name.sub_string(4);
    if model.method(relation) != () { return "$this"; }
}

// What a query of models has from the model: its scopes, and the query methods it declares itself,
// such as `withTrashed` for one that's soft deleted.
fn builder_method(type, name) {
    let args = type.type_args;
    // of a model nothing says, as with a parameter of just `Builder`: any model's scope can be called
    // on it, as a scope's own `$query` shows, and it's only known that it could be such a query
    if args.len() == 0 || args[0].class_name == model_class() {
        if laravel_has_scope(name) { return "$this"; }
        if name in ["withTrashed", "withoutTrashed", "onlyTrashed", "restore", "restoreOrCreate", "createOrRestore"]
            && class("Illuminate\\Database\\Eloquent\\SoftDeletes") != () {
            return "$this";
        }
    } else {
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
    if class_name == "Illuminate\\Support\\Optional" {
        let args = type.type_args;
        if args.len() > 0 && !args[0].is_unknown() {
            return #{ forward_to: args[0].to_string(), nullable: true };
        }
        return "mixed";
    }
    let c = class(class_name);
    if c == () { return; }
    if c.is_subtype_of(model_class()) { return "mixed"; }
    // a request's input, by name
    if c.is_subtype_of("Illuminate\\Http\\Request") { return "mixed"; }
    // `$errors->login`, the messages of a named form
    if class_name == "Illuminate\\Support\\ViewErrorBag" {
        let bag = type.method_type("getBag");
        if bag != () { return bag.to_string(); }
    }
    // `$users->map->name`: each value's, as the values have it
    if class_name == "Illuminate\\Support\\HigherOrderCollectionProxy" {
        let args = type.type_args;
        if args.len() > 1 && args[1].class_name != () { return #{ forward_to: args[1].to_string() }; }
    }
}

// ----------------------------------------------------------------------------- return types

fn return_type_targets() {
    #{
        functions: ["optional", "view"],
        methods: [
            "Illuminate\\Contracts\\Auth\\Guard::user",
            "Illuminate\\Auth\\SessionGuard::user",
            "Illuminate\\Auth\\AuthManager::user",
            "Illuminate\\Http\\Request::user",
            "Illuminate\\Contracts\\View\\Factory::make",
            "Illuminate\\Contracts\\View\\Factory::file",
            "Illuminate\\Contracts\\View\\Factory::first",
            "Illuminate\\Http\\Client\\PendingRequest::get",
            "Illuminate\\Http\\Client\\PendingRequest::head",
            "Illuminate\\Http\\Client\\PendingRequest::post",
            "Illuminate\\Http\\Client\\PendingRequest::patch",
            "Illuminate\\Http\\Client\\PendingRequest::put",
            "Illuminate\\Http\\Client\\PendingRequest::delete",
            "Illuminate\\Http\\Client\\PendingRequest::send",
            "Illuminate\\Database\\Eloquent\\Factories\\Factory::create",
            "Illuminate\\Database\\Eloquent\\Factories\\Factory::make",
            "Illuminate\\Database\\Eloquent\\Factories\\Factory::createOne",
            "Illuminate\\Database\\Eloquent\\Factories\\Factory::makeOne",
            "Illuminate\\Database\\Eloquent\\Factories\\Factory::createMany",
            "Illuminate\\Database\\Eloquent\\Factories\\Factory::createQuietly",
            "Illuminate\\Database\\Eloquent\\Factories\\Factory::createOneQuietly",
            "Illuminate\\Database\\Eloquent\\Factories\\Factory::createManyQuietly",
            // what a model's own method returns, when it says a class and not of what
            "Illuminate\\Database\\Eloquent\\Model::*",
        ],
        static_methods: [
            "Illuminate\\Support\\Facades\\Auth::user",
            "Illuminate\\Support\\Facades\\View::make",
            "Illuminate\\Support\\Facades\\View::file",
            "Illuminate\\Support\\Facades\\View::first",
            "Illuminate\\Database\\Eloquent\\Model::factory",
            "Illuminate\\Support\\Facades\\Http::get",
            "Illuminate\\Support\\Facades\\Http::head",
            "Illuminate\\Support\\Facades\\Http::post",
            "Illuminate\\Support\\Facades\\Http::patch",
            "Illuminate\\Support\\Facades\\Http::put",
            "Illuminate\\Support\\Facades\\Http::delete",
            "Illuminate\\Support\\Facades\\Http::send",
        ],
    }
}

// An HTTP client's call gives a response, or a promise of one after `async()` on the way to it, as
// far as it can be seen. Its own PHPDoc says both, having no way to say which.
fn http_result(node, ctx) {
    let declared = ctx.declared_return_type;
    let promised = false;
    for t in declared.types() { if t.class_name == "GuzzleHttp\\Promise\\PromiseInterface" { promised = true; } }
    if !promised { return; }
    let at = node.receiver;
    while at != () && at.kind == "method_call" {
        if at.name == "async" { return; }
        at = at.receiver;
    }
    if at != () && at.kind == "static_call" && at.name == "async" { return; }
    "Illuminate\\Http\\Client\\Response"
}

// The view Laravel's factory makes, which is more than the contract it's declared to return.
fn made_view(ctx) {
    if ctx.declared_return_type.class_name == "Illuminate\\Contracts\\View\\View" { return "Illuminate\\View\\View"; }
}

fn return_type_of_function(node, ctx) {
    if node.name == "view" { return made_view(ctx); }
    // `optional($value)`, knowing what it's the optional of
    let args = node.arguments;
    if args.len() != 1 { return; }
    let value = ctx.get_type(args[0]);
    if value.is_unknown() || value.is_null() { return; }
    "Illuminate\\Support\\Optional<" + value.without_null().to_string() + ">"
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
    let on = ctx.get_type(node.receiver).class_name;
    let c = if on != () { class(on) } else { () };
    if c != () {
        if c.is_subtype_of(factory_class()) { return factory_result(node, c); }
        if c.is_subtype_of(model_class()) {
            let declared = ctx.declared_return_type.class_name;
            if declared == "Illuminate\\Database\\Eloquent\\Collection" {
                return collection_of_relation(c, node.name);
            }
            let relation = class(declared);
            if relation != () && relation.is_subtype_of(relation_class()) {
                return relation_with_models(c, node.name, relation);
            }
            return;
        }
        if c.is_subtype_of("Illuminate\\Contracts\\View\\Factory") { return made_view(ctx); }
        if c.name == "Illuminate\\Http\\Client\\PendingRequest" { return http_result(node, ctx); }
    }
    let user = user_model();
    if user == () { return; }
    "?" + user
}

fn return_type_of_static_method(node, ctx) {
    let name = node.name.to_lower();
    if name == "factory" {
        // what a model's `@use HasFactory<ItsFactory>` says already says more
        if ctx.declared_return_type.class_name != factory_class() { return; }
        let model = class(ctx.get_type(node.receiver).class_name);
        if model == () { return; }
        let factory = factory_of(model);
        if factory != () { return factory.name; }
        return;
    }
    if name == "make" || name == "file" || name == "first" { return made_view(ctx); }
    if name != "user" { return http_result(node, ctx); }
    let user = user_model();
    if user == () { return; }
    "?" + user
}
