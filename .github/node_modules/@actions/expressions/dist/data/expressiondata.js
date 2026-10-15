export var Kind;
(function (Kind) {
    Kind[Kind["String"] = 0] = "String";
    Kind[Kind["Array"] = 1] = "Array";
    Kind[Kind["Dictionary"] = 2] = "Dictionary";
    Kind[Kind["Boolean"] = 3] = "Boolean";
    Kind[Kind["Number"] = 4] = "Number";
    Kind[Kind["CaseSensitiveDictionary"] = 5] = "CaseSensitiveDictionary";
    Kind[Kind["Null"] = 6] = "Null";
})(Kind || (Kind = {}));
export function kindStr(k) {
    switch (k) {
        case Kind.Array:
            return "Array";
        case Kind.Boolean:
            return "Boolean";
        case Kind.Null:
            return "Null";
        case Kind.Number:
            return "Number";
        case Kind.Dictionary:
            return "Object";
        case Kind.String:
            return "String";
    }
    return "unknown";
}
//# sourceMappingURL=expressiondata.js.map