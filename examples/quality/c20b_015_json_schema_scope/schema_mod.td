// C20B-015 / ROOT-18: Defining-module side.
//
// Exports `loadUser`, which internally casts raw JSON through the local
// `UserSchema` typedef. Only `loadUser` is listed in the export facade;
// `UserSchema` is a private typedef of the defining module.
//
// Pre-fix: caller importing just `loadUser` would crash with
// "Unknown schema type 'UserSchema' for JSON casting" because the
// interpreter consulted the caller's TypeDef registry rather than the
// module where `loadUser` was defined.

UserSchema = @(name: Str, age: Int)

loadUser =
  raw <= "{\"name\":\"alice\",\"age\":30}"
  parsed <= JSON[raw, UserSchema]()
  | parsed.hasValue |> parsed.__value.name + "/" + parsed.__value.age.toString()
  | _ |> "no"
=> :Str

<<< @(loadUser)
