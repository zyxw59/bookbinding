use std::path::PathBuf;

use clap::Parser;
use lopdf::{Document, Object};

#[derive(Debug, Parser)]
struct Args {
    /// Path to the input PDF
    input: PathBuf,
    /// Path to the output PDF
    output: PathBuf,
    #[command(flatten)]
    signature_params: SignatureParams,
    /// Adds an extra page at the start and end of the document.
    #[arg(long)]
    end_pages: bool,
}

#[derive(Debug, clap::Args)]
struct SignatureParams {
    /// Preferred number of sheets per signature
    #[arg(short, long, default_value_t = 6)]
    signature_size: usize,
    /// Minimum number of sheets in the last signature. If the remainder would be less than this
    /// amount, the last signature will instead be extra-long.
    #[arg(short, long, default_value_t = 4)]
    minimum_remainder_size: usize,
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let args = Args::parse();
    let mut document = Document::load(args.input)?;
    if args.end_pages {
        add_pages(&mut document, 1, true)?;
        add_pages(&mut document, 1, false)?;
    }
    let num_pages = document.page_iter().size_hint().0;
    add_pages(&mut document, 4 - (num_pages % 4), false)?;
    let pages = document
        .page_iter()
        .map(|id| document.get_object(id).map(|obj| (id, obj.clone())))
        .collect::<Result<Vec<_>, _>>()?;
    arrange_pages_with(pages.len(), args.signature_params, |src, dest| {
        let src_obj = pages[src].1.clone();
        let dest_id = pages[dest].0;
        document.set_object(dest_id, src_obj);
    });
    document.save(args.output)?;
    Ok(())
}

/// Adds blank pages to the document. The pages will be a copy of the first page of the document
/// with all content removed.
fn add_pages(document: &mut Document, count: usize, at_start: bool) -> color_eyre::Result<()> {
    // get first page of document
    let mut page = document
        .get_object(
            document
                .page_iter()
                .next()
                .expect("document does not have any pages"),
        )?
        .clone();
    // remove the contents
    page.as_dict_mut()?.remove(b"Contents");

    let page_tree_id = document.catalog()?.get(b"Pages")?.as_reference()?;
    // pre-allocate a new node so that we can reference it later
    let new_node_id = document.add_object(Object::Null);
    match count {
        0 => return Ok(()),
        1 => {
            // this single page will go directly under the top-level page tree
            page.as_dict_mut().unwrap().set(b"Parent", page_tree_id);
            document.set_object(new_node_id, page);
        }
        _ => {
            // a new page tree node will be inserted, with all the new pages as children
            page.as_dict_mut().unwrap().set(b"Parent", new_node_id);
            let new_pages = (0..count)
                .map(|_| Object::Reference(document.add_object(page.clone())))
                .collect::<Vec<_>>();
            let new_node = Object::Dictionary(
                [
                    ("Type", Object::from("Pages")),
                    ("Parent", Object::from(page_tree_id)),
                    ("Kids", Object::from(new_pages)),
                    ("Count", Object::from(count as i64)),
                ]
                .into_iter()
                .collect(),
            );
            document.set_object(new_node_id, new_node);
        }
    };
    let page_tree = document.get_dictionary_mut(page_tree_id)?;
    // update the top-level page tree's count of pages
    let page_tree_count = page_tree.get_mut(b"Count")?;
    *page_tree_count = Object::Integer(page_tree_count.as_i64()? + count as i64);
    let kids = page_tree.get_mut(b"Kids")?.as_array_mut()?;
    // insert the new page
    if at_start {
        kids.insert(0, new_node_id.into());
    } else {
        kids.push(new_node_id.into());
    }
    Ok(())
}

/// Arrange the pages using the given parameters, using the provided function to update the pages.
/// The first argument to the function is the page index in the input document, and the second
/// argument is the page index in the output document.
fn arrange_pages_with(
    num_pages: usize,
    params: SignatureParams,
    mut with: impl FnMut(usize, usize),
) {
    let pages_per_signature = params.signature_size * 4;
    let mut num_signatures = num_pages / pages_per_signature;
    let mut remainder = num_pages - num_signatures * pages_per_signature;
    // if the remainder would be too short, make an overlong signature instead of a short
    // signature.
    if remainder > 0 && remainder <= params.minimum_remainder_size * 4 && num_signatures >= 1 {
        num_signatures -= 1;
        remainder += pages_per_signature;
    }
    for sig in 0..num_signatures {
        signature_with(sig * pages_per_signature, params.signature_size, &mut with);
    }
    signature_with(
        num_signatures * pages_per_signature,
        remainder.div_ceil(4),
        &mut with,
    );
}

/// Arrange the pages for a given signature using the given parameters, using the provided function
/// to update the pages.
/// The first argument to the function is the page index in the input document, and the second
/// argument is the page index in the output document.
fn signature_with(start: usize, num_sheets: usize, mut with: impl FnMut(usize, usize)) {
    println!("{}", start + 1);
    let num_pages = num_sheets * 4;
    let end = start + num_pages;
    for i in 0..num_sheets {
        let s = i * 2;
        let dest = start + i * 4;
        with(end - (s + 1), dest);
        with(start + s, dest + 1);
        with(start + s + 1, dest + 2);
        with(end - (s + 2), dest + 3);
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use test_case::test_case;

    #[test_case(26, 5)]
    #[test_case(36, 5)]
    #[test_case(40, 5)]
    #[test_case(40, 6)]
    fn arrange_pages(num_pages: usize, signature_size: usize) {
        let params = super::SignatureParams {
            signature_size,
            minimum_remainder_size: 4,
        };
        let mut pages = HashSet::new();
        let mut duplicates = Vec::new();
        super::arrange_pages_with(num_pages, params, |src, _dest| {
            if !pages.insert(src) {
                duplicates.push(src);
            }
        });
        let num_pages_rounded = num_pages.next_multiple_of(4);
        assert_eq!(pages.len(), num_pages_rounded);
        assert_eq!(duplicates, []);
    }

    #[test]
    fn signature() {
        let mut pages = [0; 16];
        super::signature_with(0, 4, |src, dest| {
            pages[dest] = src;
        });
        assert_eq!(
            pages,
            [15, 0, 1, 14, 13, 2, 3, 12, 11, 4, 5, 10, 9, 6, 7, 8]
        )
    }
}
