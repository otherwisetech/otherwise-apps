# catalogue
core.catalog.json = Impossible de lire le catalogue : {detail}
core.catalog.schema = Le catalogue utilise le format {n}, que cette version ne comprend pas. Mettez à jour Otherwise Apps.
core.catalog.invalid = L’entrée « {id} » du catalogue n’est pas valide ({why}).
# archives
core.archive.io = Impossible de décompresser le téléchargement : {detail}
core.archive.format = Le téléchargement n’est pas une archive valide : {detail}
core.archive.unsafe = Téléchargement refusé : « {path} » serait écrit hors du dossier de l’app.
core.archive.unsupported = Type de fichier inconnu : {name}
# installation
core.install.io = Impossible d’écrire {path} : {detail}
core.install.no_payload = Le téléchargement ne contient pas l’app à l’endroit attendu.
core.install.wrong_bundle = Le téléchargement contient une autre app ({found} au lieu de {expected}) ; rien n’a été installé.
core.install.not_writable = Pas l’autorisation d’installer dans {path}.
core.install.not_ours = {path} n’est pas un emplacement où Otherwise Apps installe cette app ; il n’a pas été touché.
core.install.trash = Impossible de mettre {path} à la corbeille : {detail}
# réseau
core.net.rate_limited = GitHub limite les requêtes en ce moment ; réessayez dans quelques minutes.
core.net.status = Le serveur a répondu avec l’erreur {code}.
core.net.transport = Erreur réseau : {detail}
core.net.io = Impossible d’enregistrer le téléchargement : {detail}
core.net.too_large = Le téléchargement est plus gros que prévu.
core.net.cancelled = Annulé.
core.net.size = Le téléchargement est incomplet ({got} sur {want} octets).
# vérification
core.verify.missing = GitHub n’a pas publié de somme SHA-256 pour ce fichier : il ne peut pas être vérifié et n’a pas été installé.
core.verify.mismatch = Le téléchargement ne correspond pas à la somme SHA-256 de GitHub (attendu {expected}…, reçu {actual}…). Il a été supprimé et rien n’a été installé.
