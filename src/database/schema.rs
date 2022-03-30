table! {
    bundle (id) {
        id -> Text,
        owner_address -> Text,
        block_height -> BigInt,
    }
}

table! {
    leaders (address) {
        address -> Text,
    }
}

table! {
    transactions (id) {
        id -> Text,
        epoch -> Binary,
        block_promised -> BigInt,
        block_actual -> Nullable<BigInt>,
        signature -> Binary,
        validated -> Bool,
        bundle_id -> Nullable<Text>,
    }
}

table! {
    validators (address) {
        address -> Text,
        url -> Nullable<Text>,
    }
}

joinable!(transactions -> bundle (bundle_id));

allow_tables_to_appear_in_same_query!(bundle, leaders, transactions, validators,);
