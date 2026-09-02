## DONE

- [X] delete first survey screenshot

## TODO

- [ ] the raise hands feature is sometimes not working when loading the
  presentation as the presenter. I had to reload the page to see the raised
  hands from the audience. 
- [ ] resize the cat image to make it load quicker
- [ ] fix viewport for images to the longest side. I.e. make sure that the image
  is always fully visible in the viewport, even if the browser window is very
  narrow.
- [ ] open external links in a new tab
- [ ] i want to have an "open pdf" button in the slide editor 
- [ ] there final presentation (including all user input) should be downloadable
  after the presentation is over. it should be stored in a way that it can be
  shared with others (e.g. a link to a PDF or a zip file with the slides and
  user input)
- [ ] Fuzz the Markdown format to find issues and edge-cases.
- [ ] this project has historically grown. there are a lot of inconsistencies
  across pages in the html and css and a lot of issues with the amount of
  javascript and the lack of leaning into htmx4 and the backend. do a one-over
  and streamline the UI as well as the code. we are not live yet, so you have
  free reign. Use exa search to look for design best practices and
  accessibility.
- [ ] let's build a little linter/validator for the CLI, which checks if a
  Markdown file can be properly rendered. it should have checks for syntax and
  semantics.
- [ ] Would be cool to have keyboard shortcuts for raise hand feature and perhaps some other common operations
- [ ] show notes / presenter notes 
- [ ] audience chat for questions with upvotes
- [ ] the Markdown editor should have syntax-hihighlighting
- [ ] the slide editor should have a vertical split view with the Markdown on
  the left and the rendered slides on the right. it should be resizable by
  dragging the divider between the two panes.
- [ ] Interactive Markdown examples should actually be a cheat sheet with a list
  of all the Markdown features and how to use them. It could be a modal that can
  be opened by clicking on a little question mark icon.
- [ ] "Publish version" is confusing. I was of the impression that it would
  always publish the current version of the slides whenever I make changes. E.g.
  it would save in the background.
- [ ] "Theme and display" seems like it could be behind a "Settings" button or a
  gear icon that pops in from the right side of the screen.
- [ ] The title field above the markdown editor should be moved elsewhere, e.g.
  to the top of the page next to the "Slides" headline/logo. Maybe we use a
  little breadcrumb navigation bar with the title of the presentation and the
  current slide number. The title field could be a text input that can be edited
  in place.