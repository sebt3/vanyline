/// Bornes par défaut, centralisées et documentées. Les outils les référencent —
/// jamais de nombre magique dans un outil.
pub const READ_MAX_LINES: usize = 200;
pub const READ_MAX_BYTES: usize = 16 * 1024;
pub const LIST_MAX_ENTRIES: usize = 200;
pub const SEARCH_MAX_MATCHES: usize = 50;
pub const COMMAND_MAX_BYTES: usize = 8 * 1024;

/// Tronque `text` à `max_lines`/`max_bytes` (première limite atteinte).
/// Si tronqué, ajoute une ligne finale actionnable :
/// `[truncated — {n} more lines, call again with offset={next}]`
/// où `next` = offset + nombre de lignes retournées.
/// `offset` est l'offset (en lignes, 0-based) déjà appliqué par l'appelant,
/// utilisé uniquement pour calculer le `next` du message.
pub fn bound_lines(text: &str, offset: usize, max_lines: usize, max_bytes: usize) -> String {
    if text.is_empty() {
        return String::new();
    }

    let lines: Vec<&str> = text.lines().collect();
    let total_lines = lines.len();
    let line_limit = total_lines.min(max_lines);

    // Appliquer la limite en nombre de lignes
    let mut result: String = lines[..line_limit].join("\n");

    // Si les octets dépassent, supprimer des lignes depuis la fin
    if result.len() > max_bytes {
        for _ in 0..line_limit {
            if let Some(pos) = result.rfind('\n') {
                result.truncate(pos);
            }
            if result.len() <= max_bytes {
                break;
            }
        }
    }

    let returned_count = if result.is_empty() { 0 } else { result.lines().count() };
    let truncated = returned_count < total_lines;

    if truncated {
        let remaining = total_lines - returned_count;
        let next_offset = offset + returned_count;
        let marker = format!(
            "\n[truncated — {} more lines, call again with offset={}]",
            remaining, next_offset
        );
        format!("{result}{marker}")
    } else {
        result
    }
}

/// Bornage tête+queue pour les sorties de commandes : si `text` dépasse
/// `max_bytes`, garde la première moitié du budget au début et la seconde à la
/// fin, séparées par :
/// `[... {n} bytes truncated ...]`
/// La coupure se fait sur des frontières de lignes (jamais au milieu d'une ligne).
pub fn bound_head_tail(text: &str, max_bytes: usize) -> String {
    if text.is_empty() || text.len() <= max_bytes {
        return text.to_string();
    }

    let lines: Vec<&str> = text.lines().collect();
    let total_bytes = text.len();
    let marker = format!("[... {} bytes truncated ...]", total_bytes - max_bytes);

    // Budget restant pour le contenu : max_bytes - marker.len() - 2 newlines
    let budget = max_bytes.saturating_sub(marker.len() + 2);
    let half = budget / 2;

    // --- Header : lignes depuis le début (jusqu'à half bytes) ---
    let split_point = find_split(&lines, half);

    // --- Footer : lignes depuis la fin (jusqu'à half bytes) ---
    let footer_len = find_count_from_end(&lines, half);

    // Ajuster : footer ne doit pas empiéter sur le header
    let footer_len = footer_len.min(lines.len().saturating_sub(split_point));

    // --- Construction ---
    let header = lines[..split_point].join("\n");
    let footer_start = lines.len() - footer_len;
    let footer = if footer_start >= lines.len() {
        String::new()
    } else {
        lines[footer_start..].join("\n")
    };

    let result = format!("{header}\n{marker}\n{footer}");

    // Si dépasse, reculer footer_start (réduire le footer)
    if result.len() > max_bytes {
        let mut start = footer_start;
        loop {
            let foot = if start >= lines.len() {
                String::new()
            } else {
                lines[start..].join("\n")
            };
            let r = format!("{header}\n{marker}\n{foot}");
            if r.len() <= max_bytes || start >= lines.len() - 1 {
                return r;
            }
            start += 1;
        }
    }

    result
}

/// Trouve le nombre de lignes depuis le début qui tiennent dans `max_bytes`.
fn find_split(lines: &[&str], max_bytes: usize) -> usize {
    let mut result_len = 0;
    for (i, line) in lines.iter().enumerate() {
        // bytes = sum(len) + (count-1) * 1 pour les '\n' entre lignes
        let next_len = result_len + if i == 0 { line.len() } else { line.len() + 1 };
        if next_len > max_bytes {
            return i;
        }
        result_len = next_len;
    }
    lines.len()
}

/// Trouve le nombre de lignes depuis la fin qui tiennent dans `max_bytes`.
fn find_count_from_end(lines: &[&str], max_bytes: usize) -> usize {
    let mut result_len = 0;
    let mut count = 0;
    for line in lines.iter().rev() {
        let next_len = result_len + if count == 0 { line.len() } else { line.len() + 1 };
        if next_len > max_bytes {
            break;
        }
        result_len = next_len;
        count += 1;
    }
    count
}

/// Numérote les lignes façon `cat -n` : `{num:>5}\t{ligne}`, numérotation
/// 1-based commençant à `start_line`. Utilisé par read_file (tâche suivante).
pub fn number_lines(text: &str, start_line: usize) -> String {
    if text.is_empty() {
        return String::new();
    }
    text.lines()
        .enumerate()
        .map(|(i, line)| format!("{:>5}\t{}", start_line + i, line))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_lines_untouched() {
        let text = (0..10)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = bound_lines(&text, 0, 200, 16 * 1024);
        assert_eq!(result, text);
        assert!(!result.contains("truncated"));
    }

    #[test]
    fn bound_lines_truncates() {
        let lines: Vec<String> = (0..300)
            .map(|i| format!("line {} with some content to make it longer", i))
            .collect();
        let text = lines.join("\n");

        let result = bound_lines(&text, 0, 200, 16 * 1024);
        // Doit contenir 200 lignes + le marqueur de troncation
        let line_count = result.lines().count();
        // 200 lignes de contenu + 1 ligne de marqueur = 201
        assert_eq!(line_count, 201);
        assert!(result.contains("100 more lines"));
        assert!(result.contains("offset=200"));
        assert!(result.contains("truncated"));
    }

    #[test]
    fn bound_lines_respects_offset() {
        let lines: Vec<String> = (0..300)
            .map(|i| format!("line {}", i))
            .collect();
        let text = lines.join("\n");

        let result = bound_lines(&text, 200, 200, 16 * 1024);
        // Le `next` est offset + returned = 200 + 200 = 400
        assert!(result.contains("offset=400"));
        assert!(result.contains("truncated"));
    }

    #[test]
    fn bound_lines_byte_limit() {
        // Lignes très longues, le byte limit sera atteint avant max_lines
        let lines: Vec<String> = (0..200)
            .map(|_| "x".repeat(200)) // 200 chars per line
            .collect();
        let text = lines.join("\n");

        let result = bound_lines(&text, 0, 200, 512); // very small byte limit
        assert!(result.contains("truncated"));
        // Le byte limit doit être respecté (avec le marqueur)
        assert!(result.len() <= 512 + "[truncated — ...]\n".len() + 10);
    }

    #[test]
    fn head_tail_untouched() {
        let text = "short line 1\nshort line 2\nshort line 3";
        let result = bound_head_tail(text, 1024);
        assert_eq!(result, text);
    }

    #[test]
    fn head_tail_truncates() {
        // 100 lignes de 200 octets environ
        let lines: Vec<String> = (0..100)
            .map(|i| format!("line {}: {}", i, "x".repeat(190)))
            .collect();
        let text = lines.join("\n");

        let result = bound_head_tail(&text, 8192);

        // Doit contenir le marqueur
        assert!(result.contains("bytes truncated"));

        // Doit contenir des lignes de début et de fin
        assert!(result.starts_with("line 0:"));
        // Les lignes finissent par "xxx...xxx" (190 x)
        assert!(result.ends_with("xxx"));
        assert!(result.contains("line 99:"));

        // Résultat ≤ budget + marge d'erreur
        assert!(result.len() <= 8250);

        // Le texte doit être UTF-8 valide
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    }

    #[test]
    fn head_tail_utf8() {
        let lines: Vec<String> = (0..40)
            .map(|i| format!("Ligne {} avec des accents éàù et emoji 🎉", i))
            .collect();
        let text = lines.join("\n");

        let result = bound_head_tail(&text, 512);
        // Ne doit pas paniquer
        // Doit être UTF-8 valide
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    }

    #[test]
    fn number_lines_format() {
        let text = "alpha\nbeta\ngamma";
        let result = number_lines(text, 42);
        let expected = "   42\talpha\n   43\tbeta\n   44\tgamma";
        assert_eq!(result, expected);
    }

    #[test]
    fn empty_inputs() {
        assert_eq!(bound_lines("", 0, 10, 1024), "");
        assert_eq!(bound_head_tail("", 1024), "");
        assert_eq!(number_lines("", 10), "");
    }
}