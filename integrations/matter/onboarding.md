# Onboarding : Matter

1. Demander : où tourne le Matter server (python-matter-server ou matter.js ; port 5580 par
   défaut) et si Moli l'atteint (le port n'est pas toujours exposé : un relais local suffit).
2. Les appareils sont appairés par le serveur lui-même (Moli ne commissionne rien) ; Moli ne
   fait que les lire, sans session Matter à lui.
3. Déclarer `kind = "matter"` avec `host` et `port` ; `moli-os check-config`, redémarrer ; chaque
   nœud avec un point connu (lumière, température, humidité, éclairement, présence, contact)
   apparaît. Les pièces ne viennent pas de Matter : les poser par étiquettes.
4. `writable` (numéros de nœuds) limite ce que Moli commande ; `color_temp_k` et `models.<produit>` bornent la température de couleur (kelvin) par modèle de lampe.
