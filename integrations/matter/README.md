# Matter

- Réutilisé : le protocole WebSocket du Matter server (`start_listening`, puis
  `attribute_updated` / `node_updated` / `node_removed`) ; les points viennent d'une table
  cluster/attribut (`crates/moli-matter/src/map.rs`), sans code par marque.
- `fixtures/nodes.json` : 9 nœuds réels (lampes Wi-Fi), réduits aux attributs modélisés,
  rejoués par les tests de `map.rs`.
- Identité : numéro de nœud du serveur. Le nom vient de `NodeLabel` (sinon produit + numéro).
- Reste : les plages de température de couleur annoncées par certaines lampes
  sont fausses : bornées par profil de modèle (`models`).
